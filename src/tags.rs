//! # Tags: what programs worked out about models, kept by identity and by owner
//!
//! A tag is a word or a key and a value (`arch=qwen3`): what a program worked out about a model. Every tag has an
//! *owner*, the program that wrote it: through this library a program writes only its own tags and reads anyone's
//! (a courtesy between programs, not a protection: TAG-012). An owner's tags of a
//! model are a map, each key once, kept with the owner's *version*: a program that raises its version sees its older
//! tags as missing, and works them out again. Tags belong to a model's identity (`fingerprint`), so a model that moves
//! keeps them, and two copies of a model share them. They are a cache (TAG-008), kept in the cache folder, and may be
//! deleted at any time.
//!
//! ```text
//!   <cache folder>/tags/tfp1/<64 hex>.json     one identity's tags         (Locations::cache)
//!   {
//!     "format": "tenstag-tags/2",
//!     "identity": "tfp1:<64 hex>",
//!     "tags": {
//!       "chatrun": {"version": 2, "tags": {"arch": "qwen3", "context": "32768"}},
//!       "imagenode": {"version": 1, "tags": {"encoder": "qwen2.5-vl-7b"}}
//!     },
//!     "bytes": 639446688,                       the model's size: a hint, and what tells which models may match
//!     "seen": [{"path": "/models/m.gguf", "time": "2026-11-23T14:02:11Z"}]
//!   }
//!   <cache folder>/tags/tfp1/<64 hex>.lock     its lock, held while writing
//! ```
//!
//! Spec: tenstag specs/tags.md TAG-001 to TAG-010 and specs/tag-store.md TGS-001 to TGS-008, at the commit
//! spec-pin.toml names. Any other implementation of tenstag reads and writes the same files.
//!
//! This module does not find models or compute identities; `names` (the [`crate::Library`]) does, and calls it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::{MapAccess, Visitor};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::fingerprint::{Identity, is_hex64, now_nanos, write_replacing};

/// The tag file's `format` (TGS-003).
const FORMAT: &str = "tenstag-tags/2";
/// The format before owners (tenstag-rust 0.6.0 to 0.8.0): read as no tags, replaced at the next write (TGS-004).
const FORMAT_1: &str = "tenstag-tags/1";
/// TAG-007 (SHOULD): the most tags an owner has of a model.
const MOST_TAGS: usize = 1024;
/// TGS-003: the most paths kept in `seen`.
const MOST_SEEN: usize = 16;
/// TGS-004: a file over this size need not be read.
const MOST_FILE: u64 = 1 << 20;

/// Whether `c` is a control character of TAG-002: U+0000 to U+001F, U+007F, U+0080 to U+009F.
fn is_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

/// Whether `c` is a space TAG-002 names: U+0020 or another Unicode `White_Space` character that is not a control.
fn is_space(c: char) -> bool {
    matches!(
        c,
        ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// The key and the value (none for a word) of the tag `text`.
///
/// SPEC tenstag specs/tags.md TAG-001 (MUST): split at the first `=`. TAG-002 (MUST): a key is 1 to 256 bytes with no
/// `=`, control or space. TAG-003 (MUST): a value is 1 to 4,096 bytes with no control; it may hold spaces and `=`.
///
/// # Errors
///
/// [`Error::Tag`] saying why `text` is not a tag.
pub fn parse_tag(text: &str) -> Result<(String, Option<String>)> {
    let bad = |why: &str| Error::Tag(format!("{text:?} is not a tag: {why}"));
    let (key, value) = match text.split_once('=') {
        Some((key, value)) => (key, Some(value)),
        None => (text, None),
    };
    check_key(key).map_err(|why| bad(&why))?;
    if let Some(value) = value {
        check_value(value).map_err(|why| bad(&why))?;
    }
    Ok((key.to_string(), value.map(str::to_string)))
}

fn check_key(key: &str) -> std::result::Result<(), String> {
    if key.is_empty() || key.len() > 256 {
        return Err("a key is 1 to 256 bytes".into());
    }
    if key.chars().any(|c| c == '=' || is_control(c) || is_space(c)) {
        return Err("a key holds no space, `=` or control character".into());
    }
    Ok(())
}

fn check_value(value: &str) -> std::result::Result<(), String> {
    if value.is_empty() || value.len() > 4096 {
        return Err("a value is 1 to 4096 bytes (`key=` with nothing after is not a tag)".into());
    }
    if value.chars().any(is_control) {
        return Err("a value holds no control character".into());
    }
    Ok(())
}

/// Whether `name` is an owner name.
///
/// SPEC tenstag specs/tags.md TAG-009 (MUST): 1 to 128 bytes of lowercase ASCII letters, digits, `-`, `_`, `.` and
/// `/`, starting with a letter or a digit, a `/` only between two other characters. TAG-011 (SHOULD) asks for a
/// reverse domain name (`com.example.chatrun`) or a GitHub `user/repository` in lower case, so that no two programs
/// share one.
pub fn is_owner(name: &str) -> bool {
    let ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_./".contains(&b);
    (1..=128).contains(&name.len())
        && name.bytes().all(ok)
        && name.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && !name.ends_with('/')
        && !name.contains("//")
}

/// The value set of `value`: its items, split at `,`, trimmed of spaces, without empty ones.
///
/// SPEC tenstag specs/tags.md TAG-006 (MUST): spaces are U+0020 only.
pub fn value_set(value: &str) -> BTreeSet<&str> {
    value.split(',').map(|part| part.trim_matches(' ')).filter(|part| !part.is_empty()).collect()
}

/// One owner's tags of a model: each key once, with or without a value (TAG-004), in key order, and the owner's
/// version that wrote them (TAG-010).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnerTags {
    version: u32,
    map: BTreeMap<String, Option<String>>,
}

/// No tags, for an owner that has none.
static NONE: OwnerTags = OwnerTags { version: 0, map: BTreeMap::new() };

impl OwnerTags {
    /// The owner's version that wrote these tags.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Whether the key `key` is there, with or without a value (TAG-005).
    pub fn has(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    /// The value of `key` exactly as it was written; `None` for a word or a key not there.
    pub fn value(&self, key: &str) -> Option<&str> {
        self.map.get(key)?.as_deref()
    }

    /// The items of `key`'s value: `needs=t5xxl, clip-l` gives {`clip-l`, `t5xxl`} (TAG-006). Empty for a word or a
    /// key not there.
    pub fn value_set(&self, key: &str) -> BTreeSet<&str> {
        self.value(key).map(value_set).unwrap_or_default()
    }

    /// The words: the keys with no value.
    pub fn words(&self) -> impl Iterator<Item = &str> {
        self.map.iter().filter(|(_, v)| v.is_none()).map(|(k, _)| k.as_str())
    }

    /// Every key and its value, in key order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.map.iter().map(|(k, v)| (k.as_str(), v.as_deref()))
    }

    /// How many tags.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Whether these tags match: a key (has it), or a key and a value (has exactly that value) (TAG-005).
    pub(crate) fn matches(&self, key: &str, value: Option<&str>) -> bool {
        match value {
            None => self.has(key),
            Some(value) => self.value(key) == Some(value),
        }
    }

    /// Removes the key `key`, whatever its value (TAG-004).
    pub(crate) fn remove(&mut self, key: &str) {
        self.map.remove(key);
    }

    /// Removes every tag.
    pub(crate) fn clear(&mut self) {
        self.map.clear();
    }

    /// Sets a tag, replacing any with its key (TAG-004).
    fn set(&mut self, key: String, value: Option<String>) -> Result<()> {
        if !self.map.contains_key(&key) && self.map.len() >= MOST_TAGS {
            return Err(Error::Tag(format!("an owner has at most {MOST_TAGS} tags of a model")));
        }
        self.map.insert(key, value);
        Ok(())
    }
}

/// A model's tags, of every owner, as a program sees them: its own (when they were written at its version) and any
/// other owner's by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tags {
    owners: BTreeMap<String, OwnerTags>,
    /// The reading program's owner and version, if it gave one.
    me: Option<(String, u32)>,
}

impl Tags {
    pub(crate) fn new(owners: BTreeMap<String, OwnerTags>, me: Option<(String, u32)>) -> Tags {
        Tags { owners, me }
    }

    /// The program's own tags, written at its version; none when they were written at another version (TAG-010), or
    /// when the library was opened without an owner.
    pub fn mine(&self) -> &OwnerTags {
        match &self.me {
            Some((owner, version)) => self.owners.get(owner).filter(|t| t.version == *version).unwrap_or(&NONE),
            None => &NONE,
        }
    }

    /// The tags of the owner `owner`, whatever version wrote them ([`OwnerTags::version`] says which).
    pub fn of(&self, owner: &str) -> &OwnerTags {
        self.owners.get(owner).unwrap_or(&NONE)
    }

    /// Every owner with tags of this model, and its tags, in owner order.
    pub fn owners(&self) -> impl Iterator<Item = (&str, &OwnerTags)> {
        self.owners.iter().map(|(o, t)| (o.as_str(), t))
    }

    /// Whether the program's own tags hold `key` ([`Tags::mine`]).
    pub fn has(&self, key: &str) -> bool {
        self.mine().has(key)
    }

    /// The value of the program's own `key`, exactly as written ([`Tags::mine`]).
    pub fn value(&self, key: &str) -> Option<&str> {
        self.mine().value(key)
    }

    /// The items of the program's own `key`'s value ([`Tags::mine`]).
    pub fn value_set(&self, key: &str) -> BTreeSet<&str> {
        self.mine().value_set(key)
    }

    /// Whether no owner has tags of this model.
    pub fn is_empty(&self) -> bool {
        self.owners.values().all(OwnerTags::is_empty)
    }

    /// Every owner's words and values, as one text, for a search (names.md NAM-005).
    pub(crate) fn text(&self) -> String {
        let mut text = String::new();
        for tags in self.owners.values() {
            for (k, v) in tags.iter() {
                text.push_str(&format!(" {k} {}", v.unwrap_or("")));
            }
        }
        text
    }
}

/// One owner's object in a tag file (TGS-003): its version, and its tags, refusing a key that appears twice.
#[derive(Deserialize)]
struct Group {
    version: u32,
    tags: Strict,
}

/// An owner's `tags`, refusing a key that appears twice (TGS-004).
struct Strict(Vec<(String, Option<String>)>);

/// The file's `tags`: every owner's object, refusing an owner that appears twice (TGS-004).
struct Owners(Vec<(String, Group)>);

/// A JSON object read as its pairs, in order, refusing a key that appears twice.
fn pairs<'de, A: MapAccess<'de>, T: Deserialize<'de>>(mut map: A) -> std::result::Result<Vec<(String, T)>, A::Error> {
    let mut pairs: Vec<(String, T)> = Vec::new();
    while let Some((key, value)) = map.next_entry::<String, T>()? {
        if pairs.iter().any(|(k, _)| *k == key) {
            return Err(serde::de::Error::custom(format!("{key:?} appears twice")));
        }
        pairs.push((key, value));
    }
    Ok(pairs)
}

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Strict, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of tags")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> std::result::Result<Strict, A::Error> {
                pairs(map).map(Strict)
            }
        }
        d.deserialize_map(V)
    }
}

impl<'de> Deserialize<'de> for Owners {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Owners, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Owners;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of owners")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> std::result::Result<Owners, A::Error> {
                pairs(map).map(Owners)
            }
        }
        d.deserialize_map(V)
    }
}

/// The parts of a tag file this code checks (TGS-003); the rest is kept as it is.
#[derive(Deserialize)]
struct Checked {
    format: String,
    identity: String,
    tags: Owners,
}

/// What reading a tag file found.
enum Read {
    /// No file, or a file of format 1, which is read as none and replaced (TGS-004, TGS-005).
    Missing,
    /// A good file: every owner's tags, and the whole file to keep its other keys.
    Good(BTreeMap<String, OwnerTags>, Map<String, Value>),
    /// A file of a format this code does not know, never changed.
    Other(String),
    /// A file that is not a good `tenstag-tags/2` file, with why.
    Bad(String),
}

/// The tag folder of one cache folder.
pub(crate) struct Store {
    folder: PathBuf,
}

impl Store {
    /// The store of the cache folder `cache` (TGS-001): its tags are in `tags/tfp1/` (TGS-002).
    pub(crate) fn new(cache: &Path) -> Store {
        Store { folder: cache.join("tags").join("tfp1") }
    }

    fn file(&self, identity: &Identity) -> PathBuf {
        self.folder.join(format!("{}.json", identity.hex()))
    }

    /// SPEC tenstag specs/tag-store.md TGS-004 (MUST): a missing file, a file of format 1, or one that is not good,
    /// has no tags; a file over 1 MiB is not read.
    fn read_file(&self, path: &Path, hex: &str) -> Read {
        let text = match std::fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Missing,
            Ok(m) if m.len() > MOST_FILE => return Read::Bad("it is over 1 MiB".into()),
            _ => match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(e) => return Read::Bad(e.to_string()),
            },
        };
        let whole = match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => map,
            Ok(_) => return Read::Bad("it is not a JSON object".into()),
            Err(e) => return Read::Bad(e.to_string()),
        };
        match whole.get("format").and_then(Value::as_str) {
            Some(FORMAT_1) => return Read::Missing,
            Some(format) if format != FORMAT => return Read::Other(format.to_string()),
            _ => {}
        }
        let checked: Checked = match serde_json::from_str(&text) {
            Ok(c) => c,
            Err(e) => return Read::Bad(e.to_string()),
        };
        if checked.format != FORMAT || checked.identity != format!("tfp1:{hex}") {
            return Read::Bad("its identity is not its name".into());
        }
        let mut owners = BTreeMap::new();
        for (owner, group) in checked.tags.0 {
            if !is_owner(&owner) {
                return Read::Bad(format!("{owner:?} is not an owner name"));
            }
            let mut tags = OwnerTags { version: group.version, map: BTreeMap::new() };
            for (key, value) in group.tags.0 {
                if let Err(why) = check_key(&key).and_then(|()| value.as_deref().map_or(Ok(()), check_value)) {
                    return Read::Bad(format!("{owner}: {key:?}: {why}"));
                }
                tags.map.insert(key, value);
            }
            owners.insert(owner, tags);
        }
        Read::Good(owners, whole)
    }

    /// Every owner's tags of `identity`; none when it has no file, or a file that cannot be read as one (with a
    /// warning).
    pub(crate) fn tags(&self, identity: &Identity) -> BTreeMap<String, OwnerTags> {
        let path = self.file(identity);
        match self.read_file(&path, identity.hex()) {
            Read::Good(owners, _) => owners,
            Read::Missing => BTreeMap::new(),
            Read::Other(format) => {
                log::warn!("{}: of format {format}, which this version does not read", path.display());
                BTreeMap::new()
            }
            Read::Bad(why) => {
                log::warn!("{}: not a tag file ({why}); taken as no tags", path.display());
                BTreeMap::new()
            }
        }
    }

    /// Every identity that has tags, with every owner's tags and its size (`bytes`, 0 when the file gives none).
    pub(crate) fn all(&self) -> Vec<(Identity, BTreeMap<String, OwnerTags>, u64)> {
        let Ok(list) = std::fs::read_dir(&self.folder) else { return Vec::new() };
        let mut out = Vec::new();
        for item in list.filter_map(|e| e.ok()) {
            let name = item.file_name().to_string_lossy().to_string();
            let Some(hex) = name.strip_suffix(".json").filter(|h| is_hex64(h)) else { continue };
            if let Read::Good(owners, whole) = self.read_file(&item.path(), hex) {
                let bytes = whole.get("bytes").and_then(Value::as_u64).unwrap_or(0);
                out.extend(Identity::parse(&format!("tfp1:{hex}")).map(|id| (id, owners, bytes)));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Changes the owner `owner`'s tags of `identity` with `change`, written at `version`, and records the model's
    /// size, the path it was seen at, and its SHA-256 when known.
    ///
    /// SPEC tenstag specs/tag-store.md TGS-005 (MUST): under the identity's lock; a file of format 1 is replaced; a
    /// file of a format this code does not know is never changed; one that is not good is renamed aside and kept;
    /// only this owner's object is changed, started from none when another version wrote it (tags.md TAG-010); an
    /// owner left with no tags is removed, and a file left with no owners deleted; else written to a temporary file
    /// and renamed over it. Unknown top-level keys and other owners' objects are kept as they are (TGS-003). The lock
    /// file stays.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn change(
        &self,
        identity: &Identity,
        owner: &str,
        version: u32,
        bytes: u64,
        seen: Option<&str>,
        sha256: Option<(&str, &str)>,
        change: impl FnOnce(&mut OwnerTags) -> Result<()>,
    ) -> Result<()> {
        std::fs::create_dir_all(&self.folder).map_err(|e| Error::io(&self.folder, e))?;
        // 1. The lock.
        let lock_path = self.folder.join(format!("{}.lock", identity.hex()));
        let lock = File::create(&lock_path).map_err(|e| Error::io(&lock_path, e))?;
        lock.lock().map_err(|e| Error::io(&lock_path, e))?;
        // 2. The file as it is now.
        let target = self.file(identity);
        let (owners, mut whole) = match self.read_file(&target, identity.hex()) {
            Read::Good(owners, whole) => (owners, whole),
            Read::Missing => (BTreeMap::new(), Map::new()),
            Read::Other(format) => {
                return Err(Error::Tag(format!(
                    "{}: of format {format}, written by a newer program; not changed",
                    target.display()
                )));
            }
            Read::Bad(why) => {
                let aside = target.with_extension(format!("json.corrupt-{}-{}", std::process::id(), now_nanos()));
                log::warn!("{}: not a tag file ({why}); kept as {}", target.display(), aside.display());
                std::fs::rename(&target, &aside).map_err(|e| Error::io(&target, e))?;
                (BTreeMap::new(), Map::new())
            }
        };
        // 3. This owner's tags only, from none when another version wrote them.
        let mut mine = owners.get(owner).filter(|t| t.version == version).cloned().unwrap_or_default();
        mine.version = version;
        change(&mut mine)?;
        let mut object = match whole.remove("tags") {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        };
        if mine.is_empty() {
            object.remove(owner);
        } else {
            let tags: Map<String, Value> =
                mine.map.iter().map(|(k, v)| (k.clone(), v.clone().map_or(Value::Null, Value::String))).collect();
            object.insert(owner.to_string(), serde_json::json!({"version": version, "tags": tags}));
        }
        // 4. Deleted when no owner has tags, else replaced.
        if object.is_empty() {
            return match std::fs::remove_file(&target) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(Error::io(&target, e)),
                _ => Ok(()),
            };
        }
        let mut seen_list: Vec<Value> = whole.get("seen").and_then(Value::as_array).cloned().unwrap_or_default();
        if let Some(path) = seen {
            seen_list.retain(|s| s.get("path").and_then(Value::as_str) != Some(path));
            seen_list.insert(0, serde_json::json!({"path": path, "time": rfc3339_now()}));
            seen_list.truncate(MOST_SEEN);
        }
        whole.insert("format".into(), FORMAT.into());
        whole.insert("identity".into(), identity.as_str().into());
        whole.insert("tags".into(), Value::Object(object));
        whole.insert("bytes".into(), bytes.into());
        whole.insert("seen".into(), Value::Array(seen_list));
        if let Some((sha256, from)) = sha256 {
            set_sha256(&mut whole, sha256, from);
        }
        let text = serde_json::to_string_pretty(&Value::Object(whole)).map_err(|e| Error::io(&target, e.into()))?;
        write_replacing(&target, text.as_bytes()).map_err(|e| Error::io(&target, e))?;
        // 5. The lock is released when `lock` is dropped.
        drop(lock);
        Ok(())
    }

    /// Records a full SHA-256 in `identity`'s tag file, when it has one (TGS-008), with where it came from.
    pub(crate) fn record_sha256(&self, identity: &Identity, sha256: &str, from: &str) -> Result<()> {
        if !self.file(identity).is_file() {
            return Ok(());
        }
        let target = self.file(identity);
        let lock_path = self.folder.join(format!("{}.lock", identity.hex()));
        let lock = File::create(&lock_path).map_err(|e| Error::io(&lock_path, e))?;
        lock.lock().map_err(|e| Error::io(&lock_path, e))?;
        if let Read::Good(_, mut whole) = self.read_file(&target, identity.hex()) {
            set_sha256(&mut whole, sha256, from);
            let text = serde_json::to_string_pretty(&Value::Object(whole)).map_err(|e| Error::io(&target, e.into()))?;
            write_replacing(&target, text.as_bytes()).map_err(|e| Error::io(&target, e))?;
        }
        drop(lock);
        Ok(())
    }

    /// Deletes temporary files left over an hour; returns how many.
    ///
    /// SPEC tenstag specs/tag-store.md TGS-006 (MAY). Tag files themselves are left: a cache, but one a program fills
    /// again only by reading models.
    pub(crate) fn clean(&self) -> usize {
        let Ok(list) = std::fs::read_dir(&self.folder) else { return 0 };
        let mut deleted = 0;
        for item in list.filter_map(|e| e.ok()) {
            let name = item.file_name().to_string_lossy().to_string();
            if !name.split_once(".json.tmp-").is_some_and(|(hex, _)| is_hex64(hex)) {
                continue;
            }
            let old = item
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().unwrap_or_default() > std::time::Duration::from_secs(3600));
            if old && std::fs::remove_file(item.path()).is_ok() {
                deleted += 1;
            }
        }
        deleted
    }
}

/// Sets `sha256` and `sha256_from` in a tag file, unless it has a SHA-256 read from the bytes and this one is only the
/// store's.
///
/// SPEC tenstag specs/tag-store.md TGS-008 (SHOULD): a `read` value is never replaced by a `store` one.
fn set_sha256(whole: &mut Map<String, Value>, sha256: &str, from: &str) {
    if from == "store" && whole.get("sha256_from").and_then(Value::as_str) == Some("read") {
        return;
    }
    whole.insert("sha256".into(), sha256.into());
    whole.insert("sha256_from".into(), from.into());
}

/// Sets the tag `text` on `tags` (TAG-004: replacing its key).
pub(crate) fn apply(tags: &mut OwnerTags, text: &str) -> Result<()> {
    let (key, value) = parse_tag(text)?;
    tags.set(key, value)
}

/// Now, in RFC 3339 UTC to the second (`2026-11-23T14:02:11Z`).
fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = ((secs / 86400) as i64, secs % 86400);
    // Howard Hinnant's days-to-civil algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest % 3600 / 60, rest % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir;

    fn vectors(name: &str) -> Value {
        let text = match name {
            "tags" => include_str!("../tests/vectors/tags.json"),
            _ => include_str!("../tests/vectors/tag-store.json"),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn tags_and_owners_parse_as_the_specification_says() {
        let v = vectors("tags");
        for case in v["parse"].as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            match parse_tag(text) {
                Ok((key, value)) => {
                    assert!(case.get("error").is_none(), "{text:?} should not be a tag");
                    assert_eq!(key, case["key"].as_str().unwrap());
                    assert_eq!(value.as_deref(), case["value"].as_str());
                }
                Err(_) => assert!(case.get("error").is_some(), "{text:?} should be a tag"),
            }
        }
        for case in v["owners"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            assert_eq!(is_owner(name), case["owner"].as_bool().unwrap(), "{name:?}");
        }
        for case in v["versions"].as_array().unwrap() {
            let written = OwnerTags {
                version: case["written"].as_u64().unwrap() as u32,
                map: BTreeMap::from([("k".to_string(), None)]),
            };
            let tags = Tags::new(
                BTreeMap::from([("me".to_string(), written)]),
                Some(("me".into(), case["reading"].as_u64().unwrap() as u32)),
            );
            assert_eq!(tags.has("k"), case["sees"].as_bool().unwrap(), "{case}");
            assert!(tags.of("me").has("k"), "another reader sees them whatever their version");
        }
    }

    #[test]
    fn an_owners_key_has_one_value_read_exactly_or_as_a_set() {
        let v = vectors("tags");
        for case in v["value_set"].as_array().unwrap() {
            let want: Vec<&str> = case["set"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
            let got: Vec<&str> = case["value"].as_str().map(|v| value_set(v).into_iter().collect()).unwrap_or_default();
            assert_eq!(got, want, "{}", case["value"]);
        }
        let mut tags = OwnerTags::default();
        for step in v["replace"].as_array().unwrap() {
            apply(&mut tags, step["set"].as_str().unwrap()).unwrap();
            let want: Vec<(String, Option<String>)> = step["tags"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string)))
                .collect();
            let got: Vec<(String, Option<String>)> =
                tags.iter().map(|(k, v)| (k.to_string(), v.map(str::to_string))).collect();
            assert_eq!(got, want, "after {}", step["set"]);
        }
        let mut tags = OwnerTags::default();
        apply(&mut tags, "needs=t5xxl, clip-l").unwrap();
        apply(&mut tags, "chat").unwrap();
        assert_eq!(tags.value("needs"), Some("t5xxl, clip-l"));
        assert!(tags.value_set("needs").contains("clip-l"));
        assert_eq!(tags.words().collect::<Vec<_>>(), ["chat"]);
        assert!(tags.matches("needs", None) && !tags.matches("needs", Some("t5xxl")));
    }

    #[test]
    fn tag_files_are_read_as_the_specification_says() {
        let dir = test_dir("tag-files");
        let store = Store::new(&dir);
        std::fs::create_dir_all(&store.folder).unwrap();
        for case in vectors("tag-store")["files"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let path = store.folder.join(format!("{name}.json"));
            std::fs::write(&path, case["text"].as_str().unwrap()).unwrap();
            let got = match store.read_file(&path, name) {
                Read::Good(owners, _) => Some(owners),
                _ => None,
            };
            let want = case["tags"].as_object().map(|owners| {
                owners
                    .iter()
                    .map(|(o, g)| {
                        let map = g["tags"]
                            .as_object()
                            .unwrap()
                            .iter()
                            .map(|(k, v)| (k.clone(), v.as_str().map(str::to_string)))
                            .collect();
                        (o.clone(), OwnerTags { version: g["version"].as_u64().unwrap() as u32, map })
                    })
                    .collect::<BTreeMap<_, _>>()
            });
            assert_eq!(got, want, "{}", case["why"]);
        }
    }

    #[test]
    fn an_owner_changes_only_its_own_tags_and_a_new_version_starts_afresh() {
        let dir = test_dir("tag-writes");
        let store = Store::new(&dir);
        let id = Identity::parse(&format!("tfp1:{}", "ab".repeat(32))).unwrap();
        let path = store.file(&id);
        // A file of format 1 is replaced.
        std::fs::create_dir_all(&store.folder).unwrap();
        std::fs::write(&path, r#"{"format": "tenstag-tags/1", "identity": "x", "tags": {"old": null}}"#).unwrap();
        store.change(&id, "chatrun", 1, 5, Some("/m/a.gguf"), None, |t| apply(t, "arch=qwen3")).unwrap();
        store
            .change(&id, "imagenode", 1, 5, Some("/m/b.gguf"), Some(("ab".repeat(32).as_str(), "store")), |t| {
                apply(t, "encoder=t5xxl")
            })
            .unwrap();
        let mut file: Map<String, Value> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["format"], "tenstag-tags/2");
        assert_eq!(file["tags"]["chatrun"], serde_json::json!({"version": 1, "tags": {"arch": "qwen3"}}));
        assert_eq!(file["seen"][0]["path"], "/m/b.gguf");
        assert_eq!(file["seen"][1]["path"], "/m/a.gguf");
        assert!(file["seen"][0]["time"].as_str().unwrap().ends_with('Z'));
        file.insert("x-other".into(), 1.into());
        std::fs::write(&path, serde_json::to_string(&file).unwrap()).unwrap();

        // chatrun at version 2: its version-1 tags are gone at its first write; imagenode's are left alone.
        store.change(&id, "chatrun", 2, 5, None, None, |t| apply(t, "context=32768")).unwrap();
        let owners = store.tags(&id);
        assert_eq!(owners["chatrun"].iter().collect::<Vec<_>>(), [("context", Some("32768"))]);
        assert_eq!(owners["chatrun"].version(), 2);
        assert_eq!(owners["imagenode"].value("encoder"), Some("t5xxl"));
        let file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(file["x-other"], 1);

        // An owner left with no tags is removed; no owners left: no file.
        store
            .change(&id, "chatrun", 2, 5, None, None, |t| {
                t.clear();
                Ok(())
            })
            .unwrap();
        assert!(!store.tags(&id).contains_key("chatrun"));
        store
            .change(&id, "imagenode", 1, 5, None, None, |t| {
                t.remove("encoder");
                Ok(())
            })
            .unwrap();
        assert!(!path.exists(), "no tags left: no file");

        std::fs::write(&path, "{").unwrap();
        store.change(&id, "chatrun", 2, 5, None, None, |t| apply(t, "chat")).unwrap();
        assert!(store.tags(&id)["chatrun"].has("chat"));
        let aside = std::fs::read_dir(&store.folder)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".json.corrupt-"));
        assert!(aside, "the bad file is kept for a person to look at");

        std::fs::write(&path, r#"{"format": "tenstag-tags/3"}"#).unwrap();
        assert!(
            store.change(&id, "chatrun", 2, 5, None, None, |t| apply(t, "x")).is_err(),
            "a newer format is never changed"
        );
    }

    #[test]
    fn the_date_is_rfc_3339() {
        let now = rfc3339_now();
        assert_eq!(now.len(), 20);
        assert!(now.starts_with("20") && now.as_bytes()[10] == b'T');
    }
}
