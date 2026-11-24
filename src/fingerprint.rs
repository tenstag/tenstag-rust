//! # Fingerprints: a model's identity from its contents
//!
//! A model's identity is a fingerprint of its bytes, so that a model moved, renamed or copied keeps its tags. The
//! fingerprint `tfp1` reads at most about 33 MiB of a file, whatever its size: the first 16 MiB, the last 1 MiB, and
//! one 4 KiB block from each of 4,096 even runs between them. A model of several files (a split set, a model
//! directory, a pipeline) is identified by its files' fingerprints together.
//!
//! ```text
//!   Entry::files()  ->  members (TFP-007)  ->  each file: the cache (path, size, time; or file id, size, time)
//!                                                         else read and hashed (TFP-003, TFP-004)
//!                                          ->  one identity: the member's, or the set's hash of them
//! ```
//!
//! Spec: tenstag specs/fingerprint.md TFP-001 to TFP-007 and specs/fingerprint-cache.md TFC-001 to TFC-005, at the
//! commit spec-pin.toml names. The cache is shared with every other implementation of tenstag.
//!
//! This module reads files only to hash them; what a model is, it never looks at.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::scan::Entry;

/// SPEC tenstag specs/fingerprint.md TFP-002 (MUST): the constants.
const HEAD: u64 = 16 << 20;
const TAIL: u64 = 1 << 20;
const BLOCK: u64 = 4096;
const SAMPLES: u64 = 4096;
const WHOLE: u64 = HEAD + TAIL + SAMPLES * BLOCK;

/// The cache file's `format` (TFC-002).
const CACHE_FORMAT: &str = "tenstag-fingerprints/1";

/// A model's identity: `tfp1:` and 64 lowercase hexadecimal digits (TFP-001). Two models with the same identity are
/// the same model, wherever they are, and share their tags.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Identity(String);

impl Identity {
    /// The identity as text, `tfp1:<hex>`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The identity of `text` when it is one (`tfp1:` and 64 lowercase hexadecimal digits).
    pub fn parse(text: &str) -> Option<Identity> {
        let hex = text.strip_prefix("tfp1:")?;
        is_hex64(hex).then(|| Identity(text.to_string()))
    }

    /// The 64 hexadecimal digits, which name the identity's tag file.
    pub(crate) fn hex(&self) -> &str {
        &self.0["tfp1:".len()..]
    }

    fn of_digest(digest: &[u8]) -> Identity {
        Identity(format!("tfp1:{}", hex(digest)))
    }

    fn digest(&self) -> Vec<u8> {
        (0..32).map(|i| u8::from_str_radix(&self.hex()[2 * i..2 * i + 2], 16).unwrap_or(0)).collect()
    }
}

impl std::fmt::Display for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether `text` is 64 lowercase hexadecimal digits.
pub(crate) fn is_hex64(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SPEC tenstag specs/fingerprint.md TFP-005 (MUST): SplitMix64.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// The offsets of the blocks sampled from a file of `size` bytes, which must be more than `WHOLE`, in file order.
///
/// SPEC tenstag specs/fingerprint.md TFP-004 (MUST): `M` whole blocks after the head; one block from each of
/// `SAMPLES` runs, `lo + r mod (hi - lo)`. The products are taken in 128 bits so that none wraps.
fn sample_offsets(size: u64) -> Vec<u64> {
    let m = u128::from((size - HEAD - TAIL) / BLOCK);
    let mut rng = SplitMix64(size);
    (0..u128::from(SAMPLES))
        .map(|i| {
            let lo = i * m / u128::from(SAMPLES);
            let hi = (i + 1) * m / u128::from(SAMPLES);
            let block = lo + u128::from(rng.next()) % (hi - lo);
            // At most M - 1 < 2^52, so it fits.
            HEAD + block as u64 * BLOCK
        })
        .collect()
}

/// The fingerprint digest of the file at `path`, and its size.
///
/// SPEC tenstag specs/fingerprint.md TFP-003 (MUST): a file of at most `WHOLE` bytes is hashed whole after `tfp1` and
/// its size. TFP-004 (MUST): a larger one as its head, its samples, then its tail. TFP-006 (MUST): links followed (the
/// caller resolves them); a file shorter than its size, or whose size changes while it is read, has no fingerprint.
fn digest_file(path: &Path) -> std::io::Result<([u8; 32], u64)> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hash = Sha256::new();
    hash.update(b"tfp1");
    hash.update(size.to_le_bytes());
    let mut buffer = vec![0u8; 1 << 20];
    let mut copy = |file: &mut File, mut length: u64, hash: &mut Sha256| -> std::io::Result<()> {
        while length > 0 {
            let n = length.min(buffer.len() as u64) as usize;
            file.read_exact(&mut buffer[..n])?;
            hash.update(&buffer[..n]);
            length -= n as u64;
        }
        Ok(())
    };
    if size <= WHOLE {
        copy(&mut file, size, &mut hash)?;
    } else {
        copy(&mut file, HEAD, &mut hash)?;
        for offset in sample_offsets(size) {
            file.seek(SeekFrom::Start(offset))?;
            copy(&mut file, BLOCK, &mut hash)?;
        }
        file.seek(SeekFrom::Start(size - TAIL))?;
        copy(&mut file, TAIL, &mut hash)?;
    }
    if file.metadata()?.len() != size {
        return Err(std::io::Error::other("the file changed while it was read"));
    }
    Ok((hash.finalize().into(), size))
}

/// The fingerprint of the file at `path`, read now, without the cache.
///
/// # Errors
///
/// [`Error::Io`] when the file cannot be read whole, or changes while it is read.
pub fn fingerprint_file(path: &Path) -> Result<Identity> {
    digest_file(path).map(|(d, _)| Identity::of_digest(&d)).map_err(|e| Error::io(path, e))
}

/// Whether a file of a model of several files, with this name, is one of the members its identity is made of.
///
/// SPEC tenstag specs/fingerprint.md TFP-007 (MUST): not a name ending in `.md`, starting with `LICENSE` or `LICENCE`
/// (any case), or starting with `.`.
pub(crate) fn is_member(name: &str) -> bool {
    let lower = name.to_lowercase();
    !(lower.ends_with(".md") || lower.starts_with("license") || lower.starts_with("licence") || name.starts_with('.'))
}

/// The identity of a model whose members have the identities `members`.
///
/// SPEC tenstag specs/fingerprint.md TFP-007 (MUST): none with no members; the member's with one; else the SHA-256
/// of `tfp1-set`, the count, and the members' digests sorted.
pub(crate) fn set_identity(members: &[Identity]) -> Option<Identity> {
    match members {
        [] => None,
        [one] => Some(one.clone()),
        _ => {
            let mut digests: Vec<Vec<u8>> = members.iter().map(Identity::digest).collect();
            digests.sort();
            let mut hash = Sha256::new();
            hash.update(b"tfp1-set");
            hash.update((digests.len() as u64).to_le_bytes());
            digests.iter().for_each(|d| hash.update(d));
            Some(Identity::of_digest(&hash.finalize()))
        }
    }
}

/// The members of `entry`'s model (TFP-007): its files without model cards, licences and hidden files.
pub(crate) fn members(entry: &Entry) -> Vec<PathBuf> {
    entry.files().into_iter().filter(|p| p.file_name().is_some_and(|n| is_member(&n.to_string_lossy()))).collect()
}

/// What the file system says of a file now: what a cache row is matched on (TFC-003).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stat {
    path: String,
    bytes: u64,
    modified: String,
    file_id: Option<String>,
}

impl Stat {
    /// The stat of `path`, links resolved.
    fn of(path: &Path) -> std::io::Result<Stat> {
        let real = std::fs::canonicalize(path)?;
        let meta = std::fs::metadata(&real)?;
        let modified = match meta.modified()?.duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_nanos().to_string(),
            Err(e) => format!("-{}", e.duration().as_nanos()),
        };
        Ok(Stat { path: path_text(&real), bytes: meta.len(), modified, file_id: file_id(&meta) })
    }
}

/// A path as the cache and tag files write it.
///
/// DECISION: on Windows `canonicalize` gives `\\?\C:\...`; the prefix is dropped where the path is an ordinary drive
/// path, so that other programs read the path they expect.
pub(crate) fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => rest.to_string(),
        _ => text.to_string(),
    }
}

/// SPEC tenstag specs/fingerprint-cache.md TFC-002 (MUST): `<st_dev>:<st_ino>` on Unix.
///
/// DECISION: none on Windows yet. The volume serial number and file index need `GetFileInformationByHandle`, which
/// std does not offer stably; without them a file moved on Windows is read again, which costs time only.
#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    Some(format!("{}:{}", meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_: &std::fs::Metadata) -> Option<String> {
    None
}

/// One row of the cache (TFC-002), with the keys this code does not know kept as they were.
#[derive(Clone, Debug)]
struct Row {
    stat: Stat,
    identity: Identity,
    previous: Option<Identity>,
    /// The file's SHA-256 and where it came from (`store` or `read`, TFC-002).
    sha256: Option<(String, String)>,
    other: Map<String, Value>,
}

impl Row {
    fn from_json(value: &Value) -> Option<Row> {
        let object = value.as_object()?;
        let text = |k: &str| object.get(k).and_then(Value::as_str).map(str::to_string);
        let stat = Stat {
            path: text("path")?,
            bytes: object.get("bytes")?.as_u64()?,
            modified: text("modified")?,
            file_id: text("file_id"),
        };
        let identity = Identity::parse(&text("identity")?)?;
        let previous = text("previous").and_then(|p| Identity::parse(&p));
        let sha256 = text("sha256").filter(|h| is_hex64(h)).zip(text("sha256_from"));
        let mut other = object.clone();
        for key in ["path", "bytes", "modified", "file_id", "identity", "previous", "sha256", "sha256_from"] {
            other.remove(key);
        }
        Some(Row { stat, identity, previous, sha256, other })
    }

    fn to_json(&self) -> Value {
        let mut object = self.other.clone();
        object.insert("path".into(), self.stat.path.clone().into());
        object.insert("bytes".into(), self.stat.bytes.into());
        object.insert("modified".into(), self.stat.modified.clone().into());
        if let Some(id) = &self.stat.file_id {
            object.insert("file_id".into(), id.clone().into());
        }
        object.insert("identity".into(), self.identity.as_str().into());
        if let Some(previous) = &self.previous {
            object.insert("previous".into(), previous.as_str().into());
        }
        if let Some((sha256, from)) = &self.sha256 {
            object.insert("sha256".into(), sha256.clone().into());
            object.insert("sha256_from".into(), from.clone().into());
        }
        Value::Object(object)
    }
}

/// The fingerprint cache of one cache folder: the rows read, and the rows found since, to write.
pub(crate) struct Cache {
    folder: PathBuf,
    rows: Option<HashMap<String, Row>>,
    /// Rows to write, by path, with the path a moved file's row had before (to drop it).
    changed: Vec<(Row, Option<String>)>,
}

impl Cache {
    pub(crate) fn new(folder: &Path) -> Cache {
        Cache { folder: folder.to_path_buf(), rows: None, changed: Vec::new() }
    }

    fn file(&self) -> PathBuf {
        self.folder.join("fingerprints.json")
    }

    /// The rows on disk, by path; none when the file is missing, unreadable or of another format (TFC-004 step 2).
    fn read_rows(&self) -> (HashMap<String, Row>, Map<String, Value>) {
        let file = std::fs::read_to_string(self.file())
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .filter(|f| f.get("format").and_then(Value::as_str) == Some(CACHE_FORMAT))
            .unwrap_or_default();
        let rows = file
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Row::from_json)
            .map(|r| (r.stat.path.clone(), r))
            .collect();
        (rows, file)
    }

    fn rows(&mut self) -> &mut HashMap<String, Row> {
        if self.rows.is_none() {
            self.rows = Some(self.read_rows().0);
        }
        self.rows.get_or_insert_with(HashMap::new)
    }

    /// The identity of the file at `path`: from the cache when it may be (TFC-003), else read and remembered.
    pub(crate) fn file_identity(&mut self, path: &Path) -> Result<Identity> {
        let stat = Stat::of(path).map_err(|e| Error::io(path, e))?;
        let same = |r: &Row| r.stat.bytes == stat.bytes && r.stat.modified == stat.modified;
        if let Some(row) = self.rows().get(&stat.path).filter(|r| same(r)).cloned() {
            // A row written before store names were read (0.6.0) gets the store's SHA-256 once.
            if row.sha256.is_none()
                && let Some(sha256) = store_sha256(Path::new(&stat.path))
            {
                self.remember(Row { sha256: Some((sha256, "store".into())), ..row.clone() }, None);
            }
            return Ok(row.identity);
        }
        // TFC-003: moved or renamed on the same file system: the same file id, size and time.
        let moved = stat
            .file_id
            .as_ref()
            .and_then(|id| self.rows().values().find(|r| r.stat.file_id.as_ref() == Some(id) && same(r)).cloned());
        if let Some(row) = moved {
            log::debug!("{}: moved from {}; not read again", stat.path, row.stat.path);
            let old_path = row.stat.path.clone();
            let new_row = Row { stat: stat.clone(), ..row };
            self.remember(new_row, Some(old_path));
            return Ok(self.rows()[&stat.path].identity.clone());
        }
        let (digest, size) = digest_file(Path::new(&stat.path)).map_err(|e| Error::io(path, e))?;
        if size != stat.bytes {
            return Err(Error::io(path, std::io::Error::other("the file changed while it was read")));
        }
        let identity = Identity::of_digest(&digest);
        // TFC-004 step 3: a path whose contents changed keeps the identity it had before.
        let previous = self
            .rows()
            .get(&stat.path)
            .map(|old| if old.identity == identity { old.previous.clone() } else { Some(old.identity.clone()) })
            .unwrap_or(None);
        let sha256 = store_sha256(Path::new(&stat.path)).map(|h| (h, "store".to_string()));
        let row = Row { stat, identity: identity.clone(), previous, sha256, other: Map::new() };
        self.remember(row, None);
        Ok(identity)
    }

    /// The identity of the file at `path` when a row may give it (TFC-003), without reading the file or writing.
    pub(crate) fn known(&mut self, path: &Path) -> Option<Identity> {
        let stat = Stat::of(path).ok()?;
        let same = |r: &Row| r.stat.bytes == stat.bytes && r.stat.modified == stat.modified;
        if let Some(row) = self.rows().get(&stat.path).filter(|r| same(r)) {
            return Some(row.identity.clone());
        }
        let id = stat.file_id.as_ref()?;
        self.rows().values().find(|r| r.stat.file_id.as_ref() == Some(id) && same(r)).map(|r| r.identity.clone())
    }

    /// The SHA-256 the cache knows for the file at `path`, and where it came from, while its row is current.
    pub(crate) fn sha256(&mut self, path: &Path) -> Option<(String, String)> {
        let stat = Stat::of(path).ok()?;
        let row = self.rows().get(&stat.path)?;
        (row.stat.bytes == stat.bytes && row.stat.modified == stat.modified).then(|| row.sha256.clone()).flatten()
    }

    /// Records a SHA-256 computed from the bytes of the file at `path`, whose row is current (TFC-006: it replaces
    /// one from the store's name).
    pub(crate) fn set_read_sha256(&mut self, path: &Path, sha256: &str) {
        let Ok(stat) = Stat::of(path) else { return };
        let Some(row) = self.rows().get(&stat.path).cloned() else { return };
        if row.stat == stat {
            self.remember(Row { sha256: Some((sha256.to_string(), "read".into())), ..row }, None);
        }
    }

    /// The identity `path` had before its contents last changed, if the cache knows one.
    pub(crate) fn previous(&mut self, path: &Path) -> Option<Identity> {
        let real = std::fs::canonicalize(path).ok()?;
        self.rows().get(&path_text(&real)).and_then(|r| r.previous.clone())
    }

    fn remember(&mut self, row: Row, moved_from: Option<String>) {
        let rows = self.rows();
        if let Some(old) = &moved_from {
            rows.remove(old);
        }
        rows.insert(row.stat.path.clone(), row.clone());
        self.changed.push((row, moved_from));
    }

    /// Writes the rows found since the last write.
    ///
    /// SPEC tenstag specs/fingerprint-cache.md TFC-004 (MUST): under the lock, the file read again and the rows
    /// merged into it, written to a temporary file and renamed over it. A write that fails is logged, not returned:
    /// the fingerprints are still right, and are computed again next time.
    pub(crate) fn save(&mut self) {
        if self.changed.is_empty() {
            return;
        }
        let changed = std::mem::take(&mut self.changed);
        if let Err(e) = self.write(&changed) {
            log::debug!("{}: the fingerprint cache was not written ({e})", self.folder.display());
        }
    }

    fn write(&self, changed: &[(Row, Option<String>)]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.folder)?;
        let lock = File::create(self.folder.join("fingerprints.lock"))?;
        lock.lock()?;
        let (mut rows, mut file) = self.read_rows();
        for (row, moved_from) in changed {
            if let Some(old) = moved_from {
                rows.remove(old);
            }
            rows.insert(row.stat.path.clone(), row.clone());
        }
        let mut list: Vec<&Row> = rows.values().collect();
        list.sort_by(|a, b| a.stat.path.cmp(&b.stat.path));
        file.insert("format".into(), CACHE_FORMAT.into());
        file.insert("files".into(), Value::Array(list.into_iter().map(Row::to_json).collect()));
        let text = serde_json::to_string_pretty(&Value::Object(file)).map_err(std::io::Error::other)?;
        write_replacing(&self.file(), text.as_bytes())?;
        drop(lock);
        Ok(())
    }

    /// Drops the rows of files that are gone from folders that are still there; returns how many.
    ///
    /// SPEC tenstag specs/fingerprint-cache.md TFC-005 (MAY): a row of a path that cannot be reached is kept.
    pub(crate) fn clean(&mut self) -> std::io::Result<usize> {
        std::fs::create_dir_all(&self.folder)?;
        let lock = File::create(self.folder.join("fingerprints.lock"))?;
        lock.lock()?;
        let (mut rows, mut file) = self.read_rows();
        let before = rows.len();
        rows.retain(|path, _| {
            let path = Path::new(path);
            !(matches!(std::fs::metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
                && path.parent().is_some_and(Path::is_dir))
        });
        let dropped = before - rows.len();
        if dropped > 0 {
            let mut list: Vec<&Row> = rows.values().collect();
            list.sort_by(|a, b| a.stat.path.cmp(&b.stat.path));
            file.insert("format".into(), CACHE_FORMAT.into());
            file.insert("files".into(), Value::Array(list.into_iter().map(Row::to_json).collect()));
            let text = serde_json::to_string_pretty(&Value::Object(file)).map_err(std::io::Error::other)?;
            write_replacing(&self.file(), text.as_bytes())?;
        }
        self.rows = Some(rows);
        Ok(dropped)
    }
}

/// The SHA-256 the store names the file at the real path `path` by, or `None`.
///
/// SPEC tenstag specs/fingerprint-cache.md TFC-006 (SHOULD): a Hugging Face cache blob of 64 lowercase hex digits in
/// `models--*/blobs/` (tenstag specs/stores/hf-hub-cache.md HFC-020: the Hub's statement, not checked by the cache); an
/// Ollama blob `blobs/sha256-<hex>` whose `blobs` folder has a `manifests` folder beside it (specs/stores/ollama.md
/// OLM-009); a Docker Model Runner blob `blobs/sha256/<hex>` (specs/stores/docker-model-runner.md DMR-007). Never a
/// 40-digit git blob, `sha512/`, or RamaLama's blobs, which have `refs` and `snapshots` beside them rather than
/// `manifests` (specs/stores/ramalama.md RML-006: the name is sometimes the hash of a name).
pub(crate) fn store_sha256(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let parent = path.parent()?;
    let grand = parent.parent()?;
    let (parent_name, grand_name) = (parent.file_name()?.to_str()?, grand.file_name()?.to_str()?);
    if parent_name == "blobs" && grand_name.starts_with("models--") && is_hex64(name) {
        return Some(name.to_string());
    }
    if let Some(hex) = name.strip_prefix("sha256-").filter(|h| is_hex64(h))
        && parent_name == "blobs"
        && grand.join("manifests").is_dir()
    {
        return Some(hex.to_string());
    }
    (parent_name == "sha256" && grand_name == "blobs" && is_hex64(name)).then(|| name.to_string())
}

/// `bytes` written to a temporary file beside `target`, flushed, and renamed over it; on Windows the rename is
/// retried for a short while, as a reader may hold `target` open.
pub(crate) fn write_replacing(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let unique = format!("{}-{}", std::process::id(), now_nanos());
    let name = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let temporary = target.with_file_name(format!("{name}.tmp-{unique}"));
    let mut out = File::create(&temporary)?;
    out.write_all(bytes).and_then(|()| out.sync_all())?;
    drop(out);
    let mut tries = 0;
    loop {
        match std::fs::rename(&temporary, target) {
            Ok(()) => return Ok(()),
            Err(_) if cfg!(windows) && tries < 20 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(e);
            }
        }
    }
}

pub(crate) fn now_nanos() -> u128 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

/// The full SHA-256 of the file at `path`, with `progress` told the bytes read so far and the size.
pub(crate) fn sha256_file(path: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<String> {
    let mut file = File::open(path).map_err(|e| Error::io(path, e))?;
    let size = file.metadata().map_err(|e| Error::io(path, e))?.len();
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 4 << 20];
    let mut done = 0u64;
    loop {
        let n = file.read(&mut buffer).map_err(|e| Error::io(path, e))?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        done += n as u64;
        progress(done, size);
    }
    Ok(hex(&hash.finalize()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_dir;

    /// The bytes of a generated test file: tenstag tools/reference.py `content`.
    pub(crate) fn content(seed: u64, size: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(size as usize + 32);
        let mut i = 0u64;
        while (out.len() as u64) < size {
            let mut h = Sha256::new();
            h.update(seed.to_le_bytes());
            h.update(i.to_le_bytes());
            out.extend_from_slice(&h.finalize());
            i += 1;
        }
        out.truncate(size as usize);
        out
    }

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../tests/vectors/fingerprint.json")).unwrap()
    }

    #[test]
    fn splitmix64_is_the_published_generator() {
        let v = vectors();
        let mut rng = SplitMix64(v["splitmix64"]["seed"].as_u64().unwrap());
        for want in v["splitmix64"]["first"].as_array().unwrap() {
            assert_eq!(rng.next().to_string(), want.as_str().unwrap());
        }
    }

    #[test]
    fn every_file_vector_of_the_specification_gives_its_identity() {
        let dir = test_dir("fingerprint-vectors");
        for v in vectors()["files"].as_array().unwrap() {
            let size = v["size"].as_u64().unwrap();
            let mut data = content(v["seed"].as_u64().unwrap(), size);
            if let Some(edit) = v.get("edit") {
                data[edit["offset"].as_u64().unwrap() as usize] = edit["byte"].as_u64().unwrap() as u8;
            }
            let path = dir.join("file");
            std::fs::write(&path, &data).unwrap();
            assert_eq!(fingerprint_file(&path).unwrap().as_str(), v["identity"].as_str().unwrap(), "{}", v["name"]);
            if let Some(samples) = v.get("samples") {
                let offsets = sample_offsets(size);
                let first: Vec<u64> =
                    samples["first"].as_array().unwrap().iter().map(|o| o.as_u64().unwrap()).collect();
                assert_eq!(offsets[..4], first[..], "{}", v["name"]);
            }
        }
    }

    #[test]
    fn sets_and_members_follow_the_specification() {
        let v = vectors();
        for set in v["sets"].as_array().unwrap() {
            let members: Vec<Identity> = set["members"]
                .as_array()
                .unwrap()
                .iter()
                .map(|m| Identity::parse(m.as_str().unwrap()).unwrap())
                .collect();
            assert_eq!(
                set_identity(&members).map(|i| i.0),
                set["identity"].as_str().map(str::to_string),
                "{}",
                set["why"]
            );
        }
        for m in v["members"].as_array().unwrap() {
            assert_eq!(is_member(m["name"].as_str().unwrap()), m["member"].as_bool().unwrap(), "{}", m["name"]);
        }
    }

    #[test]
    fn store_names_give_a_sha256_only_where_the_specification_says() {
        let v: Value = serde_json::from_str(include_str!("../tests/vectors/store-digests.json")).unwrap();
        let dir = test_dir("store-digests");
        for case in v["paths"].as_array().unwrap() {
            let path = dir.join(case["path"].as_str().unwrap().trim_start_matches('/'));
            let blobs_holder = path.parent().unwrap().parent().unwrap();
            for beside in case["beside"].as_array().unwrap() {
                std::fs::create_dir_all(blobs_holder.join(beside.as_str().unwrap())).unwrap();
            }
            assert_eq!(store_sha256(&path).as_deref(), case["sha256"].as_str(), "{}", case["why"]);
        }
    }

    #[test]
    fn the_cache_reads_a_file_once_and_follows_it_when_it_moves() {
        let dir = test_dir("fingerprint-cache");
        let a = dir.join("a.bin");
        std::fs::write(&a, b"hello").unwrap();
        let mut cache = Cache::new(&dir.join("cache"));
        let id = cache.file_identity(&a).unwrap();
        cache.save();
        // Another process: the row is used.
        let mut other = Cache::new(&dir.join("cache"));
        assert_eq!(other.file_identity(&a).unwrap(), id);
        #[cfg(unix)]
        {
            let b = dir.join("b.bin");
            std::fs::rename(&a, &b).unwrap();
            let mut moved = Cache::new(&dir.join("cache"));
            assert_eq!(moved.file_identity(&b).unwrap(), id, "found by its file id");
            moved.save();
            let text = std::fs::read_to_string(dir.join("cache/fingerprints.json")).unwrap();
            assert!(text.contains("b.bin") && !text.contains("a.bin"), "{text}");
            // Its contents change: a new identity, and the old one remembered.
            std::thread::sleep(std::time::Duration::from_millis(10));
            std::fs::write(&b, b"changed").unwrap();
            let new = moved.file_identity(&b).unwrap();
            assert_ne!(new, id);
            assert_eq!(moved.previous(&b), Some(id));
        }
    }
}
