//! # Scanning locations: what models each one holds
//!
//! A location is walked once, and each model found becomes an [`Entry`]: a name (its *id*), the path a reader opens,
//! and what kind of model it is. Each store has its own file, following its store document (tenstag specs/stores/):
//!
//! ```text
//!   Kind::Directory   any tree, to the location's depth (DEFAULT_DEPTH unless configured):
//!                       some/dir/model-Q4_K_M.gguf       a GGUF file (a split one by its first part)
//!                       some/dir/name/{config.json, *.safetensors}   a Hugging Face model directory
//!                       models--org--name/               the Hugging Face cache's layout (hf_cache.rs,
//!                                                        specs/stores/hf-hub-cache.md)
//!                       manifest=owner=repo=tag.json     names for llama.cpp's old flat files (llama_cpp.rs,
//!                                                        specs/stores/llama-cpp-cache.md)
//!   Kind::LmStudio    <publisher>/<model>/<file>.gguf    (lm_studio.rs, specs/stores/lm-studio.md)
//!   Kind::Ollama      manifests-v2/, manifests/, blobs/  (ollama.rs, specs/stores/ollama.md)
//! ```
//!
//! Spec: tenstag specs/listing.md LST-001 to LST-019 (each store file cites its store document too).
//!
//! The same file reached twice (two locations, or a link) is one entry, under its first id. Nothing is opened for
//! writing. What a model *is* (its architecture, its size) is for the programs that read models to record
//! (`meta`).

mod docker_models;
mod gpt4all;
mod hf_cache;
mod jan;
mod llama_cpp;
mod lm_studio;
mod modelscope;
mod ollama;
mod ramalama;
pub(crate) mod single;

use std::path::{Path, PathBuf};

use crate::files::{GgufName, is_gguf_model};
use crate::locations::{Kind, Location};

/// What a model is stored as. More formats may be added in a minor version, so a `match` needs a `_` arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Format {
    /// A GGUF file (for a split model, `path` is the first part).
    Gguf,
    /// A Hugging Face model directory: config.json, safetensors weights and tokenizer files.
    HfDirectory,
    /// A model a store holds in another form, listed so that it is not mistaken for GGUF ([`Entry::note`] says what
    /// it is): Ollama's safetensors models (specs/stores/ollama.md OLM-007, SUG-041).
    Other,
    /// A single `.safetensors` (or `.sft`) file, or for a set split as `<base>-00001-of-00003.safetensors` with no
    /// model directory around it, its first part.
    Safetensors,
    /// A single PyTorch checkpoint (`.ckpt`, `.pt`, `.pth`, `.bin`): Python pickles. Listed, never opened; a program
    /// that loads one runs whatever code it holds.
    Checkpoint,
    /// A diffusers pipeline directory: `model_index.json`, with each component (`unet/`, `transformer/`, `vae/`,
    /// `text_encoder/`, ...) in its own folder.
    Diffusers,
    /// An adapter directory as PEFT writes it (`adapter_config.json` and `adapter_model.safetensors`): a LoRA or
    /// another adapter, used with the model it was trained on.
    Adapter,
}

// DECISION (the user's, 2026-11-23): tenstag does not say what kind of model an entry is (language, image, an encoder):
// it never reads a model, and a guess from formats and folders was wrong both ways (an image tool's GGUF UNets are not
// language models; its text encoders may be). An entry gives facts (its format, its path, its id below the
// location), and the program decides, and may record what it decided as a tag. Nor does tenstag read image tools'
// role folders (`loras/`, `vae/`): their files are listed as they are, and what the folder means is the program's.
// `Class` (0.3.0 to 0.6.1) was removed in 0.7.0 for this reason.

/// Where a model in the Hugging Face cache came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubOrigin {
    /// The repository, `org/name`.
    pub repo: String,
    /// The commit of the snapshot.
    pub commit: String,
    /// The branches, tags and short commits (`refs/`) that point at this commit, `main` first.
    pub refs: Vec<String>,
    /// The model file in the repository (empty for a directory).
    pub file: String,
}

/// A model found in a location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its name in listings, unique among the entries.
    pub id: String,
    /// What a reader opens: the GGUF file (its first part), or the model directory.
    pub path: PathBuf,
    /// What it is stored as.
    pub format: Format,
    /// The quant tag in a GGUF file's name (`Q4_K_M`), or the one its store records; or empty.
    pub quant: String,
    /// The bytes of its weights: every part of a split GGUF, every safetensors file of a directory.
    pub bytes: u64,
    /// The location it was found in.
    pub location: PathBuf,
    /// Its repository and commit when it was found in the Hugging Face cache layout.
    pub hub: Option<HubOrigin>,
    /// Every part, in order, of a split GGUF model whose files do not carry the `-NNNNN-of-MMMMM` names (Ollama's
    /// blobs); empty otherwise.
    pub parts: Vec<PathBuf>,
    /// What the entry is when [`Entry::format`] is [`Format::Other`]; empty otherwise.
    pub note: String,
}

/// Every model in `locations`, in their order. A file a location holds that an earlier location already gave (the
/// same directory reached twice, or a link) is left out; within one location every name is kept, since a store
/// may give one file several names (Ollama's aliases).
pub fn scan(locations: &[Location]) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for location in locations.iter().filter(|l| l.path.is_dir()) {
        let mut found = Vec::new();
        match location.kind {
            Kind::Directory => Walk::new(location).dir(&location.path, 0, &mut found),
            Kind::LmStudio => lm_studio::scan(&location.path, &mut found),
            Kind::Ollama => ollama::scan(&location.path, &mut found),
            Kind::Gpt4All => gpt4all::scan(&location.path, location.depth.unwrap_or(8), &mut found),
            Kind::ModelScope => modelscope::scan(&location.path, &mut found),
            Kind::Jan => jan::scan(&location.path, location.depth.unwrap_or(8), &mut found),
            Kind::DockerModels => docker_models::scan(&location.path, &mut found),
            Kind::RamaLama => ramalama::scan(&location.path, &mut found),
        }
        let key = |e: &Entry| std::fs::canonicalize(&e.path).unwrap_or_else(|_| e.path.clone());
        found.retain(|e| e.format == Format::Other || !seen.contains(&key(e)));
        seen.extend(found.iter().filter(|e| e.format != Format::Other).map(key));
        for mut entry in found {
            entry.location = location.path.clone();
            // Two locations can hold different models under the same relative name: the later one's id says
            // where it is.
            if entries.iter().any(|e| e.id.eq_ignore_ascii_case(&entry.id)) {
                entry.id = format!("{}:{}", location.path.display(), entry.id);
            }
            entries.push(entry);
        }
    }
    entries
}

/// The entry for a path given directly: a model file or a model directory, or `None` when it is neither. A file
/// given by its path is taken whatever its size.
pub fn entry_for_path(path: &Path) -> Option<Entry> {
    let name = path.to_string_lossy().to_string();
    if path.is_dir() {
        if let Some(entry) = directory_entry(name.clone(), path) {
            return Some(entry);
        }
        let mut entry = new_entry(name, path.to_path_buf(), Format::HfDirectory);
        entry.bytes = safetensors_bytes(path);
        return Some(entry);
    }
    if !path.is_file() {
        return None;
    }
    if name.ends_with(".gguf") {
        let mut entry = new_entry(name.trim_end_matches(".gguf").to_string(), path.to_path_buf(), Format::Gguf);
        entry.quant = GgufName::parse(&name).map(|n| n.tag).unwrap_or_default();
        entry.bytes = gguf_bytes(path);
        return Some(entry);
    }
    let format = single::format_of(&file_name(path), true)?;
    let mut entry = new_entry(name, path.to_path_buf(), format);
    entry.bytes = single::bytes(path);
    Some(entry)
}

/// The entry of a directory whose layout says what it is, `None` for any other directory.
///
/// DECISION: the first of `model_index.json` (a diffusers pipeline), `config.json` (a model directory),
/// `adapter_config.json` (an adapter) directly in it gives its kind, so that a directory has one kind, and so one
/// set of files and one identity, wherever it is found (tenstag's own rule since 0.6.0).
///
/// Only the names of files are looked at, never their contents: `model_index.json` is not read to see what the
/// pipeline makes.
fn directory_entry(id: String, dir: &Path) -> Option<Entry> {
    if dir.join("model_index.json").is_file() {
        let mut entry = new_entry(id, dir.to_path_buf(), Format::Diffusers);
        entry.bytes = component_files(dir).iter().filter(|p| is_weights_file(p)).map(|p| size(p)).sum();
        return (entry.bytes > 0).then_some(entry);
    }
    if is_model_directory(dir) {
        let mut entry = new_entry(id, dir.to_path_buf(), Format::HfDirectory);
        entry.bytes = safetensors_bytes(dir);
        return Some(entry);
    }
    if dir.join("adapter_config.json").is_file() && !dir.join("config.json").is_file() {
        let mut entry = new_entry(id, dir.to_path_buf(), Format::Adapter);
        entry.bytes = files_in(dir).iter().filter(|p| file_name(p).starts_with("adapter_model")).map(|p| size(p)).sum();
        return (entry.bytes > 0).then_some(entry);
    }
    None
}

/// The regular files directly in `dir`, sorted.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    files.sort();
    files
}

/// A diffusers pipeline's files: those directly in it, and those directly in each component folder, sorted.
fn component_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = files_in(dir);
    let mut folders: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    folders.sort();
    for folder in folders.iter().filter(|f| !file_name(f).starts_with('.')) {
        files.extend(files_in(folder));
    }
    files
}

/// Whether `path` is a weights file of a pipeline component (`*.safetensors`, `*.bin`).
fn is_weights_file(path: &Path) -> bool {
    let name = file_name(path);
    name.ends_with(".safetensors") || name.ends_with(".bin")
}

/// The size of a file, 0 when it cannot be read.
fn size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

impl Entry {
    /// Every file of the model, as its store names them: a model file, the parts of a split one in order, a
    /// directory's files, or a diffusers pipeline's files and its components' files. Found from names and
    /// listings only; nothing is read.
    pub fn files(&self) -> Vec<PathBuf> {
        if !self.parts.is_empty() {
            return self.parts.clone();
        }
        match self.format {
            Format::Gguf => {
                let name = self.path.to_string_lossy();
                match GgufName::parse(&name) {
                    Some(gguf) if gguf.count > 1 => (1..=gguf.count).map(|i| PathBuf::from(gguf.part(i))).collect(),
                    _ => vec![self.path.clone()],
                }
            }
            Format::HfDirectory | Format::Adapter => files_in(&self.path),
            Format::Diffusers => component_files(&self.path),
            Format::Safetensors => single::parts(&self.path),
            Format::Checkpoint => vec![self.path.clone()],
            Format::Other => Vec::new(),
        }
    }
}

pub(crate) fn new_entry(id: String, path: PathBuf, format: Format) -> Entry {
    Entry {
        id,
        path,
        format,
        quant: String::new(),
        bytes: 0,
        location: PathBuf::new(),
        hub: None,
        parts: Vec::new(),
        note: String::new(),
    }
}

/// The relative path of `path` under `root`, with `/` between parts.
fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// The file name of `path`, as text.
fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// The entries of a GGUF file that is the first part of its model, `None` for any other file.
fn gguf_entry(id: String, path: &Path) -> Option<Entry> {
    let file = file_name(path);
    let gguf = GgufName::parse(&file).filter(|n| n.index == 1 && is_gguf_model(&file))?;
    let mut entry = new_entry(id, path.to_path_buf(), Format::Gguf);
    (entry.quant, entry.bytes) = (gguf.tag, gguf_bytes(path));
    (entry.bytes > 0).then_some(entry)
}

/// A walk of one location: its root, how deep it goes, and the real folders already read.
struct Walk<'a> {
    root: &'a Path,
    depth: usize,
    min_size: u64,
    /// Every folder read, with links resolved: a link back up the tree is read once, so a walk cannot loop.
    visited: std::collections::HashSet<PathBuf>,
}

impl<'a> Walk<'a> {
    fn new(location: &'a Location) -> Walk<'a> {
        Walk {
            root: &location.path,
            depth: location.depth.unwrap_or(crate::locations::DEFAULT_DEPTH),
            min_size: location.min_size,
            visited: std::collections::HashSet::new(),
        }
    }

    /// Reads `dir`, `level` parts below the root (the root is 0): a model there is listed when its path below the
    /// root has at most `depth` parts (`m.gguf` 1, `org/m.gguf` and `org/m/` 2).
    fn dir(&mut self, dir: &Path, level: usize, found: &mut Vec<Entry>) {
        let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if !self.visited.insert(real) {
            return;
        }
        let root = self.root;
        let name = file_name(dir);
        if let Some(repo) = name.strip_prefix("models--").filter(|_| dir.join("snapshots").is_dir()) {
            hf_cache::repository(dir, &repo.replace("--", "/"), found);
            return;
        }
        if dir.join("model_index.json").is_file()
            || dir.join("adapter_config.json").is_file()
            || is_model_directory(dir)
        {
            found.extend(directory_entry(relative(root, dir), dir));
            return;
        }
        // A folder `level` parts below the root holds files `level + 1` parts below it: read only while that is
        // within the depth.
        if level >= self.depth {
            return;
        }
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let mut paths: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        let named = llama_cpp::manifest_names(dir);
        for path in paths {
            let file = file_name(&path);
            if file.starts_with('.') {
                continue;
            }
            if path.is_dir() {
                if !software(&path, &file) {
                    self.dir(&path, level + 1, found);
                }
            } else {
                if file.ends_with(".gguf") {
                    let id = named
                        .get(&file)
                        .cloned()
                        .unwrap_or_else(|| relative(root, &path).trim_end_matches(".gguf").to_string());
                    found.extend(gguf_entry(id, &path));
                } else if let Some(format) = single::format_of(&file, false) {
                    // DECISION (the user's, 2026-11-23): a loose file whose name only says weights is listed from
                    // `min_size` up (DEFAULT_MIN_SIZE, 1 MiB); a split set's parts after the first are not listed.
                    if single::first_part(&file) && single::bytes(&path) >= self.min_size {
                        let mut entry = new_entry(relative(root, &path), path.clone(), format);
                        entry.bytes = single::bytes(&path);
                        found.push(entry);
                    }
                }
            }
        }
    }
}

/// Whether the folder `path`, named `name`, holds software rather than models, and is not walked.
///
/// DECISION: JavaScript packages (`node_modules`), Python's bytecode (`__pycache__`), installed packages
/// (`site-packages`) and Python virtual environments (a folder with `pyvenv.cfg`). Their files are named like
/// weights often enough (test fixtures, bundled tokenizers) to fill a listing, and a person keeps no models there.
fn software(path: &Path, name: &str) -> bool {
    matches!(name, "node_modules" | "__pycache__" | "site-packages") || path.join("pyvenv.cfg").is_file()
}

/// Whether `dir` is a Hugging Face model directory: config.json and its weights, every shard its index names. The
/// weights are `*.safetensors` (index model.safetensors.index.json), else `pytorch_model*.bin` (index
/// pytorch_model.bin.index.json), as specs/stores/hf-hub-cache.md HFC-011 lists them. Only the index's list of
/// shard files is read, not the weights.
fn is_model_directory(dir: &Path) -> bool {
    if !dir.join("config.json").is_file() {
        return false;
    }
    for index in ["model.safetensors.index.json", "pytorch_model.bin.index.json"] {
        if let Ok(text) = std::fs::read_to_string(dir.join(index)) {
            let Ok(index) = serde_json::from_str::<serde_json::Value>(&text) else { return false };
            let shards =
                index["weight_map"].as_object().map(|m| m.values().filter_map(|v| v.as_str()).collect::<Vec<_>>());
            return shards.is_some_and(|s| !s.is_empty() && s.iter().all(|s| dir.join(s).is_file()));
        }
    }
    safetensors_bytes(dir) > 0
}

/// Whether a file name is a model directory's weights: `*.safetensors`, or `pytorch_model*.bin` (HFC-011).
pub(crate) fn is_weights(name: &str) -> bool {
    name.ends_with(".safetensors") || (name.starts_with("pytorch_model") && name.ends_with(".bin"))
}

/// The bytes of a model directory's weights.
fn safetensors_bytes(dir: &Path) -> u64 {
    let Ok(read) = std::fs::read_dir(dir) else { return 0 };
    read.filter_map(|e| e.ok())
        .filter(|e| is_weights(&e.file_name().to_string_lossy()))
        .filter_map(|e| std::fs::metadata(e.path()).ok())
        .map(|m| m.len())
        .sum()
}

/// The bytes of every part of the GGUF model whose first part is `path`, or 0 when a part is missing.
fn gguf_bytes(path: &Path) -> u64 {
    let name = path.to_string_lossy();
    let Some(gguf) = GgufName::parse(&name) else { return 0 };
    (1..=gguf.count).map(|i| std::fs::metadata(gguf.part(i)).map(|m| m.len()).ok()).sum::<Option<u64>>().unwrap_or(0)
}

/// The endings of files that downloads leave beside the real ones: huggingface_hub's and llama.cpp's partial
/// files, llama.cpp's saved ETags, the locks (SPEC tenstag specs/stores/hf-hub-cache.md HFC-006, IMPL).
const PARTIAL: [&str; 4] = [".incomplete", ".downloadInProgress", ".etag", ".lock"];

/// Every file under `dir`, relative to `root` with `/` between parts, leaving out [`PARTIAL`] files.
fn list_files(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.filter_map(|e| e.ok()) {
        let path = e.path();
        if path.is_dir() {
            list_files(root, &path, out);
        } else if path.is_file() && !PARTIAL.iter().any(|suffix| path.to_string_lossy().ends_with(suffix)) {
            out.push(relative(root, &path));
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::locations::Source;
    use crate::test_dir;

    pub(crate) fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    pub(crate) fn location(path: &Path, kind: Kind) -> Location {
        Location {
            path: path.to_path_buf(),
            kind,
            source: Source::Default,
            what: "test".into(),
            depth: kind.default_depth(),
            min_size: crate::locations::DEFAULT_MIN_SIZE,
            added_by: None,
            added: None,
        }
    }

    /// A repository folder of the Hugging Face cache, with its files as blobs and links as huggingface_hub writes
    /// them, and refs/main.
    pub(crate) fn hub_repo(cache: &Path, repo: &str, commit: &str, files: &[(&str, &[u8])]) {
        hub_snapshot(cache, repo, commit, &["main"], files);
    }

    /// A snapshot of a repository folder, with the given refs pointing at it.
    pub(crate) fn hub_snapshot(cache: &Path, repo: &str, commit: &str, refs: &[&str], files: &[(&str, &[u8])]) {
        let dir = cache.join(format!("models--{}", repo.replace('/', "--")));
        for name in refs {
            write(&dir.join("refs").join(name), commit.as_bytes());
        }
        std::fs::create_dir_all(dir.join("snapshots").join(commit)).unwrap();
        for (i, (file, bytes)) in files.iter().enumerate() {
            let blob = dir.join("blobs").join(format!("{commit}{i}"));
            write(&blob, bytes);
            let link = dir.join("snapshots").join(commit).join(file);
            std::fs::create_dir_all(link.parent().unwrap()).unwrap();
            #[cfg(unix)]
            {
                let up = "../".repeat(file.matches('/').count() + 2);
                std::os::unix::fs::symlink(format!("{up}blobs/{commit}{i}"), &link).unwrap();
            }
            #[cfg(not(unix))]
            std::fs::copy(&blob, &link).unwrap();
        }
    }

    #[test]
    fn a_walk_goes_to_its_depth_cannot_loop_and_leaves_software_folders() {
        let root = test_dir("scan-walk");
        write(&root.join("a/b/c/d/e/f/deep-Q8_0.gguf"), b"1");
        write(&root.join("a/shallow-Q8_0.gguf"), b"1");
        write(&root.join("proj/node_modules/x/m-Q8_0.gguf"), b"1");
        write(&root.join("proj/venv/pyvenv.cfg"), b"");
        write(&root.join("proj/venv/lib/m-Q8_0.gguf"), b"1");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("a/loop")).unwrap();

        let ids = |depth: usize| {
            let mut location = location(&root, Kind::Directory);
            location.depth = Some(depth);
            let mut ids: Vec<String> = scan(&[location]).into_iter().map(|e| e.id).collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(6), ["a/shallow-Q8_0"], "six levels: deep-Q8_0 is the seventh; no software folder; no loop");
        assert_eq!(ids(7), ["a/b/c/d/e/f/deep-Q8_0", "a/shallow-Q8_0"]);
    }

    #[test]
    fn image_models_are_filed_by_their_layout_and_loose_files_by_name_and_size() {
        let root = test_dir("scan-image");
        let mib = vec![0u8; 1 << 20];
        write(&root.join("sd/sd_xl_base.safetensors"), &mib);
        write(&root.join("sd/v1-5.ckpt"), &mib);
        write(&root.join("sd/tiny.safetensors"), b"x");
        write(&root.join("sd/state.pt"), &mib);
        write(&root.join("st/m-00001-of-00002.safetensors"), &mib[..600_000]);
        write(&root.join("st/m-00002-of-00002.safetensors"), &mib[..600_000]);
        write(&root.join("pipe/model_index.json"), b"{}");
        write(&root.join("pipe/unet/config.json"), b"{}");
        write(&root.join("pipe/unet/diffusion_pytorch_model.safetensors"), b"1234");
        write(&root.join("pipe/vae/diffusion_pytorch_model.safetensors"), b"56");
        write(&root.join("lora/adapter_config.json"), b"{}");
        write(&root.join("lora/adapter_model.safetensors"), b"123");

        let found = |min_size: u64| {
            let mut location = location(&root, Kind::Directory);
            location.min_size = min_size;
            let mut found: Vec<(String, Format, u64)> =
                scan(&[location]).into_iter().map(|e| (e.id, e.format, e.bytes)).collect();
            found.sort_by(|a, b| a.0.cmp(&b.0));
            found
        };
        assert_eq!(
            found(crate::locations::DEFAULT_MIN_SIZE),
            [
                ("lora".to_string(), Format::Adapter, 3),
                ("pipe".to_string(), Format::Diffusers, 6),
                ("sd/sd_xl_base.safetensors".to_string(), Format::Safetensors, 1 << 20),
                ("sd/v1-5.ckpt".to_string(), Format::Checkpoint, 1 << 20),
                ("st/m-00001-of-00002.safetensors".to_string(), Format::Safetensors, 1_200_000),
            ],
            "no .pt loose, nothing under 1 MiB, one entry for a split set, a pipeline's components inside it"
        );
        assert!(found(0).iter().any(|e| e.0 == "sd/tiny.safetensors"), "min_size = 0 lists every size");

        let pipe = scan(&[location(&root, Kind::Directory)]).into_iter().find(|e| e.id == "pipe").unwrap();
        assert_eq!(pipe.files().len(), 4, "model_index.json, unet's config and weights, vae's weights");
        assert_eq!(entry_for_path(&root.join("sd/state.pt")).map(|e| e.format), Some(Format::Checkpoint));
    }

    #[test]
    fn an_image_tools_folder_lists_files_as_they_are() {
        let root = test_dir("scan-image-tool");
        let mib = vec![0u8; 1 << 20];
        write(&root.join("unet/flux1-dev-Q4_K_S.gguf"), b"12");
        write(&root.join("text_encoders/qwen_2.5_vl_7b-Q4_K_M.gguf"), b"12");
        write(&root.join("loras/style/ink.safetensors"), &mib);
        write(&root.join("diffusers/sdxl/model_index.json"), b"{}");
        write(&root.join("diffusers/sdxl/unet/diffusion_pytorch_model.safetensors"), b"1");
        write(&root.join("lora-dir/adapter_config.json"), b"{}");
        write(&root.join("lora-dir/adapter_model.safetensors"), b"1");
        let mut found: Vec<(String, Format)> =
            scan(&[location(&root, Kind::Directory)]).into_iter().map(|e| (e.id, e.format)).collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            found,
            [
                ("diffusers/sdxl".to_string(), Format::Diffusers),
                ("lora-dir".to_string(), Format::Adapter),
                ("loras/style/ink.safetensors".to_string(), Format::Safetensors),
                ("text_encoders/qwen_2.5_vl_7b-Q4_K_M".to_string(), Format::Gguf),
                ("unet/flux1-dev-Q4_K_S".to_string(), Format::Gguf),
            ],
            "each file by its format and its path below the folder; what the folder means is the program's"
        );
    }

    #[test]
    fn plain_files_and_directories_are_found_once() {
        let root = test_dir("scan");
        write(&root.join("plain/m-Q8_0.gguf"), b"12345");
        write(&root.join("plain/mmproj-m-F16.gguf"), b"x");
        write(&root.join("plain/s-Q4_K_M-00001-of-00002.gguf"), b"12");
        write(&root.join("plain/s-Q4_K_M-00002-of-00002.gguf"), b"345");
        write(&root.join("plain/half-Q8_0-00001-of-00002.gguf"), b"12");
        write(&root.join("dirs/tiny/config.json"), b"{}");
        write(&root.join("dirs/tiny/model.safetensors"), b"1234");
        write(&root.join("dirs/half/config.json"), b"{}");
        let index = br#"{"weight_map":{"a":"m-1.safetensors","b":"m-2.safetensors"}}"#;
        write(&root.join("dirs/half/model.safetensors.index.json"), index);
        write(&root.join("dirs/half/m-1.safetensors"), b"1");
        hub_repo(&root.join("hub"), "Org/Model-GGUF", "c0ffee", &[("Model-Q8_0.gguf", b"1234")]);

        let entries = scan(&[location(&root.join("hub"), Kind::Directory), location(&root, Kind::Directory)]);
        let ids: Vec<(&str, u64)> = entries.iter().map(|e| (e.id.as_str(), e.bytes)).collect();
        assert_eq!(
            ids,
            [("Org/Model-GGUF:Q8_0", 4), ("dirs/tiny", 4), ("plain/m-Q8_0", 5), ("plain/s-Q4_K_M-00001-of-00002", 5)]
        );
        assert_eq!(entries[1].format, Format::HfDirectory);
        assert_eq!(entry_for_path(&root.join("plain/m-Q8_0.gguf")).unwrap().bytes, 5);
    }
}
