//! # Locations: where models are looked for, and where tags and fingerprints are kept
//!
//! A *location* is a directory that may hold models. Three layers give them:
//!
//! ```text
//!   the configuration file  ->  environment variables  ->  built-in defaults
//!   ($TENSTAG_CONFIG, else      TENSTAG_PATH (more          (the Hugging Face cache, llama.cpp's,
//!    $XDG_CONFIG_HOME/tenstag/  directories, ':'-separated)  LM Studio's, Ollama's and the others')
//!    config.toml, else
//!    ~/.config/tenstag/config.toml)
//! ```
//!
//! Every location is only read. One folder is written: the cache folder ([`Locations::cache`], tenstag
//! specs/fingerprint-cache.md TFC-001), which holds the fingerprints and the tags (specs/tag-store.md TGS-001); and
//! the configuration file, only when a program asks to change it ([`Locations::add_to_config`], `config_write`).
//!
//! The configuration file, all keys optional:
//!
//! ```toml
//! defaults = true                    # also look in the built-in locations
//!
//! [[location]]
//! path = "/data/models"              # a directory of GGUF files, model directories or cached repositories
//! depth = 6                          # a model's path below it has at most this many parts (DEFAULT_DEPTH)
//!
//! [[location]]
//! path = "~/.ollama/models"
//! kind = "ollama"                    # Ollama's store (manifests and blobs)
//!
//! [[location]]
//! path = "/data/lm-studio/models"
//! kind = "lmstudio"                  # an LM Studio models folder (<publisher>/<model>/<file>.gguf)
//!
//! [names]
//! qwen = "Qwen/Qwen3-0.6B-GGUF:Q8_0" # a nickname: any name tenstag accepts
//! ```
//!
//! Spec: tenstag specs/configuration.md CFG-001 to CFG-009 and CFG-014, and specs/locations.md LOC-001 to LOC-005,
//! which cite the store documents (specs/stores/) for each store's folder.
//!
//! This module says where to look; `scan` says what is there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::env::Environment;
use crate::error::{Error, Result};

/// How a location's contents are laid out. More kinds may be added in a minor version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Kind {
    /// Any tree of GGUF files, Hugging Face model directories and Hugging Face cache folders (`models--org--name`):
    /// the Hugging Face cache itself, llama.cpp's cache (with its old manifests), or a user's own directory.
    Directory,
    /// An LM Studio models folder: `<publisher>/<model>/<file>.gguf` (specs/stores/lm-studio.md).
    LmStudio,
    /// Ollama's store: manifests naming blobs (specs/stores/ollama.md).
    Ollama,
    /// GPT4All's model folder: GGUF files at any depth (specs/stores/gpt4all.md).
    Gpt4All,
    /// The ModelScope cache: `models/<owner>--<name>/snapshots/<revision>/` and its older layouts
    /// (specs/stores/modelscope-cache.md).
    ModelScope,
    /// Jan's data folder: a `model.yml` per model under `llamacpp/models/` (specs/stores/jan.md).
    Jan,
    /// Docker Model Runner's store: OCI manifests and blobs (specs/stores/docker-model-runner.md).
    DockerModels,
    /// RamaLama's store folder: `store/<transport>/<organisation>/<name>/` (specs/stores/ramalama.md).
    RamaLama,
}

impl Kind {
    /// Whether a location of this kind is walked (to its [`Location::depth`]), rather than read by a fixed layout.
    pub fn walked(self) -> bool {
        self.default_depth().is_some()
    }

    /// How deep a location of this kind is walked when the configuration does not say: [`DEFAULT_DEPTH`] for a
    /// folder of your own, 8 for an application's folder that the application itself searches to any depth
    /// (GPT4All's folder, Jan's models); `None` for a store read by its fixed layout.
    pub fn default_depth(self) -> Option<usize> {
        match self {
            Kind::Directory => Some(DEFAULT_DEPTH),
            Kind::Gpt4All | Kind::Jan => Some(APP_DEPTH),
            Kind::LmStudio | Kind::Ollama | Kind::ModelScope | Kind::DockerModels | Kind::RamaLama => None,
        }
    }
}

/// How deep an application's own folder that the application searches to any depth is walked when the configuration
/// does not say. DECISION: a bound the applications themselves do not have, deep enough for any folder a person
/// arranges by hand, which a user may change with `depth`.
const APP_DEPTH: usize = 8;

/// Which layer gave a location. More may be added in a minor version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Source {
    /// Built in.
    Default,
    /// The configuration file.
    Config,
    /// An environment variable.
    Environment,
    /// The program using tenstag ([`Locations::add`]).
    Program,
}

/// A directory that may hold models.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// The directory.
    pub path: PathBuf,
    /// How its contents are laid out.
    pub kind: Kind,
    /// Which layer gave it.
    pub source: Source,
    /// What it is, in a few words ("the Hugging Face cache (HF_HOME)").
    pub what: String,
    /// How deep a walked location is read: a model is found when its path below `path` has at most this many parts
    /// (`m.gguf` is 1, `org/m.gguf` and the model folder `org/m/` are 2). [`DEFAULT_DEPTH`] unless the configuration
    /// gives `depth`. `None` for a store read by its own fixed layout (LM Studio, Ollama), which is not walked.
    pub depth: Option<usize>,
    /// The smallest loose file, in bytes, listed from a walked location when only its name says it is a model
    /// (`.safetensors`, `.ckpt`): [`DEFAULT_MIN_SIZE`] unless the configuration gives `min_size`. GGUF files and
    /// model directories are listed whatever their size.
    pub min_size: u64,
    /// For a location in the configuration file: the program that added it, an owner name (tenstag
    /// specs/configuration.md CFG-009); `None` when a person wrote it, or the program did not say.
    pub added_by: Option<String>,
    /// For a location in the configuration file: when it was added, `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SSZ`
    /// (CFG-009).
    pub added: Option<String>,
}

impl Location {
    /// A location of `kind` at `path`, as a program gives it: walked to the kind's own depth, with the default
    /// `min_size`. The builder methods below change those.
    pub fn new(path: impl Into<PathBuf>, kind: Kind) -> Location {
        let path = path.into();
        Location {
            what: format!("given by the program ({})", path.display()),
            path,
            kind,
            source: Source::Program,
            depth: kind.default_depth(),
            min_size: DEFAULT_MIN_SIZE,
            added_by: None,
            added: None,
        }
    }

    /// A folder of the program's own (`Kind::Directory`) at `path`.
    pub fn folder(path: impl Into<PathBuf>) -> Location {
        Location::new(path, Kind::Directory)
    }

    /// Walked to `depth` parts below the folder (see [`Location::depth`]). A store read by its own layout is not
    /// walked, and this has no effect on it.
    pub fn depth(mut self, depth: usize) -> Location {
        if self.kind.walked() {
            self.depth = Some(depth);
        }
        self
    }

    /// Lists loose files from `bytes` up (see [`Location::min_size`]).
    pub fn min_size(mut self, bytes: u64) -> Location {
        self.min_size = bytes;
        self
    }

    /// Described as `what` in [`Locations::describe`].
    pub fn what(mut self, what: &str) -> Location {
        self.what = what.to_string();
        self
    }
}

/// How deep a walked location is read when the configuration does not say: a few levels, enough for a hand-organised
/// tree of models, without walking a whole disk.
pub const DEFAULT_DEPTH: usize = 6;

/// The smallest loose file listed when only its name says it is a model, when the configuration does not say: 1 MiB.
///
/// DECISION (the user's, 2026-11-23): smaller files named like weights are mostly not models (test fixtures, tiny
/// training states). A location's `min_size` lists smaller ones (textual inversion embeddings, say). The size is the
/// file system's, not read from the file.
pub const DEFAULT_MIN_SIZE: u64 = 1 << 20;

/// Every location, the nicknames, and the folders tenstag writes, as the layers give them.
#[derive(Clone, Debug)]
pub struct Locations {
    /// The locations read, in the order they are searched.
    pub read: Vec<Location>,
    /// Nicknames: a short name for any name tenstag accepts.
    pub names: BTreeMap<String, String>,
    /// The configuration file read, if there was one.
    pub config_file: Option<PathBuf>,
    /// The cache folder, which holds the fingerprints already computed and the tags (tenstag
    /// specs/fingerprint-cache.md TFC-001, specs/tag-store.md TGS-001). A program may set its own. A person may
    /// delete it at any time: everything in it is worked out again.
    pub cache: PathBuf,
    /// The environment the locations were found in.
    pub env: Environment,
}

/// The configuration file's contents.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    defaults: Option<bool>,
    #[serde(default, rename = "location")]
    locations: Vec<ConfigLocation>,
    #[serde(default)]
    names: BTreeMap<String, String>,
}

/// One `[[location]]` of the configuration file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigLocation {
    path: String,
    kind: Option<String>,
    depth: Option<usize>,
    min_size: Option<u64>,
    added_by: Option<String>,
    added: Option<toml::value::Datetime>,
    /// Removed in 0.7.0; read only to say so.
    class: Option<toml::Value>,
}

impl Locations {
    /// The locations for the process's environment.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] when the configuration file exists but cannot be read or understood.
    pub fn load() -> Result<Locations> {
        Locations::load_from(Environment::from_process())
    }

    /// The locations for `env`.
    ///
    /// # Errors
    ///
    /// As for [`Locations::load`].
    pub fn load_from(env: Environment) -> Result<Locations> {
        Locations::build(env, true)
    }

    /// No locations, for a program that gives its own folders ([`Locations::add`]) and wants nothing from the
    /// user's configuration or the default locations: no configuration file is read (a broken one is no error), and
    /// neither the built-in locations nor TENSTAG_PATH are read. The cache folder is the environment's
    /// (TENSTAG_CACHE_DIR, else the usual one), as for [`Locations::load`], so the tags are the ones other programs
    /// see.
    pub fn bare() -> Locations {
        Locations::bare_from(Environment::from_process())
    }

    /// As [`Locations::bare`], for `env`.
    // DECISION: every error `build` gives comes from a configuration file, and none is read here, so the `expect`
    // cannot fail; a `Result` would make every caller handle an error that does not happen.
    #[allow(clippy::missing_panics_doc)]
    pub fn bare_from(env: Environment) -> Locations {
        Locations::build(env, false).expect("no configuration file is read, so nothing can fail")
    }

    /// Adds a location given by the program, read after every other one. It need not be named in any
    /// configuration; its models are named, searched, described and tagged like any others, their tags kept in
    /// the same cache folder ([`Locations::cache`], which a program may also change). A model id another location
    /// already has is prefixed with the folder (`/data/app:m-Q8_0`): [`Entry::location`](crate::Entry::location)
    /// says where each was found.
    pub fn add(&mut self, location: Location) {
        self.read.push(location);
    }

    /// The locations: with the configuration file and the defaults when `use_config`, else none (`bare`).
    fn build(env: Environment, use_config: bool) -> Result<Locations> {
        let home = env.home().unwrap_or_else(|| PathBuf::from("."));
        let xdg_cache = env.path("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"));
        let config_file = env
            .path("TENSTAG_CONFIG")
            .filter(|_| use_config)
            .or_else(|| Some(config_path(&env)).filter(|p| use_config && p.is_file()));
        let config = match &config_file {
            Some(path) => read_config(path)?,
            None if use_config => Config::default(),
            None => Config { defaults: Some(false), ..Config::default() },
        };

        let mut read = Vec::new();
        for location in &config.locations {
            read.push(configured(config_file.as_deref().unwrap_or(Path::new("")), &env, location)?);
        }
        if let Some(paths) = env.var("TENSTAG_PATH").filter(|_| use_config) {
            for path in std::env::split_paths(&paths).filter(|p| !p.as_os_str().is_empty()) {
                read.push(at("TENSTAG_PATH", path, Kind::Directory, Source::Environment));
            }
        }
        if config.defaults.unwrap_or(true) {
            read.extend(default_locations(&env, &home, &xdg_cache));
        }
        // The same directory given twice is read once, under its first name.
        let mut seen = Vec::new();
        read.retain(|l| {
            let key = std::fs::canonicalize(&l.path).unwrap_or_else(|_| l.path.clone());
            let new = !seen.contains(&key);
            seen.push(key);
            new
        });

        let cache = cache_folder(&env, &home);
        Ok(Locations { read, names: config.names, config_file, cache, env })
    }

    /// The configuration file a change is written to (tenstag specs/configuration.md CFG-010 step 1): the one read,
    /// else the one that would be read, `tenstag/config.toml` in `$XDG_CONFIG_HOME` or `~/.config`, whether or not it
    /// exists.
    pub fn config_path(&self) -> PathBuf {
        config_path(&self.env)
    }

    /// Adds `location` (its path, kind, depth and `min_size`) to the configuration file, with `added_by` (an owner
    /// name, as for tags) and the time now (CFG-011). When the file lists the folder already, nothing is written:
    /// [`AddedToConfig::AlreadyListed`](crate::AddedToConfig) gives the location there. The rest of the file stays as
    /// the person wrote it. This `Locations` is not changed: [`Locations::load`] again reads the new file.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] for a path that is not absolute (or `~/...`), an `added_by` that is not an owner name, or a
    /// configuration file that cannot be read or changed safely; [`Error::Io`] when it cannot be written.
    pub fn add_to_config(&self, location: &Location, added_by: Option<&str>) -> Result<crate::AddedToConfig> {
        crate::config_write::add(&self.env, location, added_by, None)
    }

    /// Removes every `[[location]]` of the folder `path` from the configuration file (CFG-012), and returns the first
    /// as it was read, with its `added_by` and `added`; `None` when the file does not list it (a folder from
    /// TENSTAG_PATH or the defaults is not in the file: see each location's [`Location::source`]).
    ///
    /// # Errors
    ///
    /// As for [`Locations::add_to_config`].
    pub fn remove_from_config(&self, path: impl AsRef<Path>) -> Result<Option<Location>> {
        crate::config_write::remove(&self.env, path.as_ref())
    }

    /// Sets `defaults` in the configuration file: whether the built-in locations are read (CFG-013). Returns whether
    /// the file changed.
    ///
    /// # Errors
    ///
    /// As for [`Locations::add_to_config`].
    pub fn set_defaults(&self, on: bool) -> Result<bool> {
        crate::config_write::set_defaults(&self.env, on)
    }

    /// The built-in locations (tenstag specs/locations.md LOC-001), in order, whatever `defaults` says (CFG-014):
    /// what turning them on would add.
    pub fn default_locations(&self) -> Vec<Location> {
        let home = self.env.home().unwrap_or_else(|| PathBuf::from("."));
        let xdg_cache = self.env.path("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"));
        default_locations(&self.env, &home, &xdg_cache)
    }

    /// One line per location, for a person: what it is, where, which layer gave it, and whether it exists.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = vec![match &self.config_file {
            Some(path) => format!("configuration: {}", path.display()),
            None => "configuration: none (TENSTAG_CONFIG, or ~/.config/tenstag/config.toml)".to_string(),
        }];
        lines.push(format!("fingerprints and tags are cached in {}", self.cache.display()));
        for l in &self.read {
            let absent = if l.path.is_dir() { "" } else { " (absent)" };
            let source = match l.source {
                Source::Default => "default",
                Source::Config => "config",
                Source::Environment => "environment",
                Source::Program => "program",
            };
            lines.push(format!("reads {} [{source}]: {}{absent}", l.what, l.path.display()));
        }
        lines
    }
}

/// The Hugging Face cache directory, and which variable chose it.
///
/// SPEC tenstag specs/stores/hf-hub-cache.md HFC-001 (IMPL), followed as HFC-003 (SHOULD, SUG-037) suggests:
/// HF_HUB_CACHE, else HUGGINGFACE_HUB_CACHE, else `$HF_HOME/hub`, where HF_HOME defaults to
/// `$XDG_CACHE_HOME/huggingface`, else `~/.cache/huggingface`; `~` and `$VAR` in them are expanded.
fn hub_cache(env: &Environment, xdg_cache: &Path) -> (PathBuf, &'static str) {
    if let Some(path) = env.var("HF_HUB_CACHE") {
        (env.expand_all(&path), "HF_HUB_CACHE")
    } else if let Some(path) = env.var("HUGGINGFACE_HUB_CACHE") {
        (env.expand_all(&path), "HUGGINGFACE_HUB_CACHE")
    } else if env.var("HF_HOME").is_some() {
        (hf_home(env).join("hub"), "HF_HOME")
    } else {
        (xdg_cache.join("huggingface").join("hub"), "default")
    }
}

/// The cache folder, where fingerprints already computed are kept, and the tags in `tags/`.
///
/// SPEC tenstag specs/fingerprint-cache.md TFC-001 (MUST), specs/tag-store.md TGS-001 (MUST): TENSTAG_CACHE_DIR when
/// absolute; else %LOCALAPPDATA%\tenstag
/// on Windows, ~/Library/Caches/tenstag on macOS, and elsewhere $XDG_CACHE_HOME/tenstag when it is absolute, else
/// ~/.cache/tenstag.
fn cache_folder(env: &Environment, home: &Path) -> PathBuf {
    if let Some(dir) = env.path("TENSTAG_CACHE_DIR").filter(|p| p.is_absolute()) {
        return dir;
    }
    let base = if cfg!(windows) {
        env.path("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"))
    } else if cfg!(target_os = "macos") {
        home.join("Library").join("Caches")
    } else {
        env.path("XDG_CACHE_HOME").filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".cache"))
    };
    base.join("tenstag")
}

/// llama.cpp's own cache directory, and what chose it.
///
/// SPEC tenstag specs/stores/llama-cpp-cache.md LCC-001 (IMPL): LLAMA_CACHE as it is; else %LOCALAPPDATA% on Windows,
/// ~/Library/Caches on macOS, $XDG_CACHE_HOME or ~/.cache elsewhere, then `llama.cpp`. With LLAMA_CACHE set it is
/// also where llama.cpp puts its Hugging Face cache (specs/stores/hf-hub-cache.md HFC-002): read, never written
/// (HFC-003, SUG-037). A [`Kind::Directory`] reads both layouts there.
fn llama_cpp_cache(env: &Environment, home: &Path, xdg_cache: &Path) -> (PathBuf, &'static str) {
    if let Some(path) = env.path("LLAMA_CACHE") {
        return (path, "LLAMA_CACHE");
    }
    let base = if cfg!(windows) {
        env.path("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"))
    } else if cfg!(target_os = "macos") {
        home.join("Library").join("Caches")
    } else {
        xdg_cache.to_path_buf()
    };
    (base.join("llama.cpp"), "default")
}

/// LM Studio's models folder, and what chose it.
///
/// SPEC tenstag specs/stores/lm-studio.md LMS-001 (IMPL): LM Studio's home is the path in `.lmstudio-home-pointer` in
/// the home directory (links resolved), trimmed; else ~/.cache/lm-studio when it exists; else ~/.lmstudio. LMS-002
/// (IMPL): the models folder is the string `downloadsFolder` of `<home>/settings.json`, else `<home>/models`.
fn lm_studio_models(home: &Path) -> (PathBuf, &'static str) {
    let real_home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let pointer = std::fs::read_to_string(real_home.join(".lmstudio-home-pointer")).ok();
    let (studio, why) = match pointer.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        Some(path) => (PathBuf::from(path), ".lmstudio-home-pointer"),
        None if home.join(".cache").join("lm-studio").is_dir() => {
            (home.join(".cache").join("lm-studio"), "~/.cache/lm-studio")
        }
        None => (home.join(".lmstudio"), "~/.lmstudio"),
    };
    let settings = std::fs::read_to_string(studio.join("settings.json")).ok();
    let moved = settings
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["downloadsFolder"].as_str().map(PathBuf::from));
    match moved {
        Some(folder) => (folder, "settings.json downloadsFolder"),
        None => (studio.join("models"), why),
    }
}

/// GPT4All's model folder, and what chose it.
///
/// SPEC tenstag specs/stores/gpt4all.md G4A-001 (IMPL): the key `modelPath` in `[General]` of `nomic.ai/GPT4All.ini` in
/// `$XDG_CONFIG_HOME` or `~/.config` (Linux and macOS) or `%APPDATA%` (Windows). G4A-002 (IMPL): else
/// `nomic.ai/GPT4All` in `$XDG_DATA_HOME` or `~/.local/share` (Linux), `~/Library/Application Support` (macOS) or
/// `%LOCALAPPDATA%` (Windows) (G4A-Q1: inferred from Qt's documentation).
///
/// DECISION: a value in quotes is taken without them; one holding a `\` is in QSettings' escaping, which this plain
/// reader does not decode (G4A-001), so the default folder is used and a warning logged. The folder holding GPT4All's
/// executable, which GPT4All also searches, is not known to a reader and is not read.
fn gpt4all_models(env: &Environment, home: &Path) -> (PathBuf, &'static str) {
    let config = if cfg!(windows) {
        env.path("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"))
    } else if cfg!(target_os = "macos") {
        home.join(".config")
    } else {
        env.path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"))
    };
    let ini = config.join("nomic.ai").join("GPT4All.ini");
    if let Some(value) = std::fs::read_to_string(&ini).ok().and_then(|text| ini_value(&text, "General", "modelPath")) {
        let value = value.trim_matches('"');
        if value.contains('\\') {
            log::warn!("{}: modelPath {value:?} is escaped; using GPT4All's default folder", ini.display());
        } else if !value.is_empty() {
            return (PathBuf::from(value), "GPT4All.ini modelPath");
        }
    }
    let data = if cfg!(windows) {
        env.path("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"))
    } else if cfg!(target_os = "macos") {
        home.join("Library").join("Application Support")
    } else {
        env.path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"))
    };
    (data.join("nomic.ai").join("GPT4All"), "default")
}

/// The value of `key` in section `[section]` of INI text, trimmed.
fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut current = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.to_string();
        } else if current == section
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// The ModelScope cache's root, and what chose it.
///
/// SPEC tenstag specs/stores/modelscope-cache.md MSC-001 (IMPL): MODELSCOPE_CACHE when set and not empty, else
/// MS_CACHE_HOME, else `~/.cache/modelscope`, on every system, `~` expanded. The older default root,
/// `~/.cache/modelscope/hub` (MSC-004), lies inside it and is read from there.
fn modelscope_cache(env: &Environment, home: &Path) -> (PathBuf, &'static str) {
    if let Some(path) = env.var("MODELSCOPE_CACHE").filter(|v| !v.is_empty()) {
        (env.expand(&path), "MODELSCOPE_CACHE")
    } else if let Some(path) = env.var("MS_CACHE_HOME").filter(|v| !v.is_empty()) {
        (env.expand(&path), "MS_CACHE_HOME")
    } else {
        (home.join(".cache").join("modelscope"), "default")
    }
}

/// Jan's data folder, and what chose it.
///
/// SPEC tenstag specs/stores/jan.md JAN-001 (IMPL): `<data root>/Jan/settings.json`, where `<data root>` is
/// JAN_DATA_ROOT when absolute, else `$XDG_DATA_HOME` or `~/.local/share` (Linux), `~/Library/Application Support`
/// (macOS) or the Roaming AppData folder (Windows; `%APPDATA%` here, which is that folder unless redirected). JAN-002
/// (IMPL): JAN_DATA_FOLDER when set, else `data_folder` of that file, else `<data root>/Jan/data`.
fn jan_data(env: &Environment, home: &Path) -> (PathBuf, &'static str) {
    if let Some(path) = env.var("JAN_DATA_FOLDER").filter(|v| !v.is_empty()) {
        return (env.expand(&path), "JAN_DATA_FOLDER");
    }
    let root = match env.path("JAN_DATA_ROOT").filter(|p| p.is_absolute()) {
        Some(root) => root,
        None if cfg!(windows) => env.path("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming")),
        None if cfg!(target_os = "macos") => home.join("Library").join("Application Support"),
        None => env.path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share")),
    };
    let settings = std::fs::read_to_string(root.join("Jan").join("settings.json")).ok();
    let moved = settings
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["data_folder"].as_str().filter(|p| !p.is_empty()).map(PathBuf::from));
    match moved {
        Some(folder) => (folder, "settings.json data_folder"),
        None => (root.join("Jan").join("data"), "default"),
    }
}

/// Docker Model Runner's store, and what chose it.
///
/// SPEC tenstag specs/stores/docker-model-runner.md DMR-001 (IMPL): MODELS_PATH when set, else `~/.docker/models`. With
/// Docker Engine the store is inside the Docker volume `docker-model-runner-models`, which only root can normally read:
/// name it as a location to read it.
fn docker_models(env: &Environment, home: &Path) -> (PathBuf, &'static str) {
    match env.var("MODELS_PATH").filter(|v| !v.is_empty()) {
        Some(path) => (env.expand(&path), "MODELS_PATH"),
        None => (home.join(".docker").join("models"), "default"),
    }
}

/// RamaLama's store folder, and what chose it.
///
/// SPEC tenstag specs/stores/ramalama.md RML-001 (IMPL): RAMALAMA_STORE; else `store` in the `[ramalama]` table of the
/// ramalama.conf files (RAMALAMA_CONFIG when that file exists, else `/etc/ramalama`, `$XDG_DATA_HOME/ramalama` and
/// `$XDG_CONFIG_HOME/ramalama`, or `%LOCALAPPDATA%\ramalama` and `%APPDATA%\ramalama` on Windows, each
/// `ramalama.conf` then its `ramalama.conf.d/*.conf`, later ones winning); else `/var/lib/ramalama` for root, else
/// `$XDG_DATA_HOME/ramalama` or `~/.local/share/ramalama`, on every system.
///
/// DECISION: the configuration folders under Python's install prefix are not read: a reader does not know which
/// Python RamaLama runs under. Root is recognised by `USER` being `root`.
fn ramalama_store(env: &Environment, home: &Path) -> (PathBuf, &'static str) {
    if let Some(path) = env.var("RAMALAMA_STORE").filter(|v| !v.is_empty()) {
        return (env.expand(&path), "RAMALAMA_STORE");
    }
    let data = env.path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"));
    let files: Vec<PathBuf> = match env.path("RAMALAMA_CONFIG").filter(|p| p.is_file()) {
        Some(file) => vec![file],
        None => {
            let folders = if cfg!(windows) {
                vec![
                    env.path("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local")),
                    env.path("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming")),
                ]
            } else {
                vec![
                    PathBuf::from("/etc"),
                    data.clone(),
                    env.path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
                ]
            };
            let mut files = Vec::new();
            for folder in folders.iter().map(|f| f.join("ramalama")) {
                files.push(folder.join("ramalama.conf"));
                let mut more: Vec<PathBuf> = std::fs::read_dir(folder.join("ramalama.conf.d"))
                    .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
                    .unwrap_or_default();
                more.retain(|p| p.extension().is_some_and(|e| e == "conf"));
                more.sort();
                files.extend(more);
            }
            files
        }
    };
    let mut store = None;
    for file in files {
        let table = std::fs::read_to_string(&file).ok().and_then(|t| t.parse::<toml::Table>().ok());
        if let Some(path) = table.as_ref().and_then(|t| t.get("ramalama")?.get("store")?.as_str()) {
            store = Some(env.expand(path));
        }
    }
    if let Some(store) = store {
        return (store, "ramalama.conf store");
    }
    if env.var("USER").as_deref() == Some("root") {
        return (PathBuf::from("/var/lib/ramalama"), "root's default");
    }
    (data.join("ramalama"), "default")
}

/// `$HF_HOME`, defaulting as huggingface_hub defaults it: where its token file lives.
pub(crate) fn hf_home(env: &Environment) -> PathBuf {
    env.var("HF_HOME").map(|h| env.expand_all(&h)).unwrap_or_else(|| {
        let home = env.home().unwrap_or_else(|| PathBuf::from("."));
        env.path("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache")).join("huggingface")
    })
}

fn read_config(path: &Path) -> Result<Config> {
    let text =
        std::fs::read_to_string(path).map_err(|e| Error::Config { path: path.into(), message: e.to_string() })?;
    parse_config(path, &text)
}

/// The configuration `text` of the file `path`.
fn parse_config(path: &Path, text: &str) -> Result<Config> {
    toml::from_str(text).map_err(|e| Error::Config { path: path.into(), message: e.to_string() })
}

/// The locations of the configuration `text` of the file `path`, as read (CFG-002 to CFG-004, CFG-009): for
/// `config_write`, which refuses a file that breaks them.
pub(crate) fn config_text_locations(path: &Path, env: &Environment, text: &str) -> Result<Vec<Location>> {
    parse_config(path, text)?.locations.iter().map(|l| configured(path, env, l)).collect()
}

/// The configuration file a change is written to (CFG-010 step 1): TENSTAG_CONFIG, else tenstag/config.toml in
/// XDG_CONFIG_HOME or ~/.config.
pub(crate) fn config_path(env: &Environment) -> PathBuf {
    env.path("TENSTAG_CONFIG").unwrap_or_else(|| {
        let home = env.home().unwrap_or_else(|| PathBuf::from("."));
        env.path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("tenstag").join("config.toml")
    })
}

/// The built-in locations, in order.
///
/// SPEC tenstag specs/locations.md LOC-001 (MUST); configuration.md CFG-014 (MUST): listed whatever `defaults` says.
fn default_locations(env: &Environment, home: &Path, xdg_cache: &Path) -> Vec<Location> {
    let hub = hub_cache(env, xdg_cache);
    let mut read = Vec::new();
    read.push(at(&format!("the Hugging Face cache ({})", hub.1), hub.0, Kind::Directory, Source::Default));
    let (llama, why) = llama_cpp_cache(env, home, xdg_cache);
    read.push(at(&format!("llama.cpp's cache ({why})"), llama, Kind::Directory, Source::Default));
    let (lm_studio, why) = lm_studio_models(home);
    read.push(at(&format!("LM Studio's models ({why})"), lm_studio, Kind::LmStudio, Source::Default));
    let (gpt4all, why) = gpt4all_models(env, home);
    read.push(at(&format!("GPT4All's models ({why})"), gpt4all, Kind::Gpt4All, Source::Default));
    let (modelscope, why) = modelscope_cache(env, home);
    read.push(at(&format!("the ModelScope cache ({why})"), modelscope, Kind::ModelScope, Source::Default));
    let (jan, why) = jan_data(env, home);
    read.push(at(&format!("Jan's models ({why})"), jan, Kind::Jan, Source::Default));
    let (docker, why) = docker_models(env, home);
    read.push(at(&format!("Docker Model Runner's store ({why})"), docker, Kind::DockerModels, Source::Default));
    let (ramalama, why) = ramalama_store(env, home);
    read.push(at(&format!("RamaLama's store ({why})"), ramalama, Kind::RamaLama, Source::Default));
    // SPEC tenstag specs/stores/ollama.md OLM-001 (IMPL): OLLAMA_MODELS, surrounding spaces and quotes removed, else
    // <home>/.ollama/models of the user the server runs as. DECISION: the Linux service's store
    // (/usr/share/ollama/.ollama/models, the documented place on Linux) is read too, where it exists.
    let ollama = env.var("OLLAMA_MODELS").map(|v| v.trim().trim_matches(['"', '\'']).to_string());
    match ollama.filter(|v| !v.is_empty()) {
        Some(path) => {
            read.push(at("Ollama's models (OLLAMA_MODELS)", env.expand(&path), Kind::Ollama, Source::Default))
        }
        None => {
            read.push(at("Ollama's models", home.join(".ollama").join("models"), Kind::Ollama, Source::Default));
            if cfg!(target_os = "linux") {
                let service = PathBuf::from("/usr/share/ollama/.ollama/models");
                read.push(at("Ollama's models (the Linux service)", service, Kind::Ollama, Source::Default));
            }
        }
    }
    read
}

/// A location as a program or the defaults give it, with the kind's depth and the default `min_size`.
fn at(what: &str, path: PathBuf, kind: Kind, source: Source) -> Location {
    Location {
        path,
        kind,
        source,
        what: what.to_string(),
        depth: kind.default_depth(),
        min_size: DEFAULT_MIN_SIZE,
        added_by: None,
        added: None,
    }
}

/// One `[[location]]` of the file `file`, as read.
///
/// SPEC tenstag specs/configuration.md CFG-003 (MUST), CFG-004 (MUST), CFG-008 (MUST), CFG-009 (MUST).
fn configured(file: &Path, env: &Environment, location: &ConfigLocation) -> Result<Location> {
    let kind = match location.kind.as_deref() {
        None | Some("directory") => Kind::Directory,
        Some("ollama") => Kind::Ollama,
        Some("lmstudio") => Kind::LmStudio,
        Some("gpt4all") => Kind::Gpt4All,
        Some("modelscope") => Kind::ModelScope,
        Some("jan") => Kind::Jan,
        Some("docker") => Kind::DockerModels,
        Some("ramalama") => Kind::RamaLama,
        Some(other) => {
            return Err(Error::Config {
                path: file.to_path_buf(),
                message: format!(
                    "location kind {other:?}: it is \"directory\", \"lmstudio\", \"ollama\", \"gpt4all\", \"modelscope\", \"jan\", \"docker\" or \"ramalama\""
                ),
            });
        }
    };
    let mut configured = at("configured", env.expand(&location.path), kind, Source::Config);
    if let Some(depth) = location.depth {
        // A store read by its fixed layout has no depth to set: say so, rather than ignore it.
        if !kind.walked() {
            return Err(Error::Config {
                path: file.to_path_buf(),
                message: format!(
                    "{}: `depth` is for a folder that is walked; this kind of store is read by its own layout",
                    location.path
                ),
            });
        }
        configured.depth = Some(depth);
    }
    if let Some(min_size) = location.min_size {
        configured.min_size = min_size;
    }
    if location.class.is_some() {
        return Err(Error::Config {
            path: file.to_path_buf(),
            message: format!(
                "{}: `class` was removed in tenstag 0.7.0: what kind of model a file is, is for the program using it to decide",
                location.path
            ),
        });
    }
    if let Some(owner) = &location.added_by
        && !crate::tags::is_owner(owner)
    {
        return Err(Error::Config {
            path: file.to_path_buf(),
            message: format!(
                "{}: `added_by` is the owner name of the program that added it (tags.md TAG-009), not {owner:?}",
                location.path
            ),
        });
    }
    configured.added_by.clone_from(&location.added_by);
    configured.added = location.added.as_ref().map(|added| added_text(file, &location.path, added)).transpose()?;
    Ok(configured)
}

/// SPEC tenstag specs/configuration.md CFG-009 (MUST): a local date as `YYYY-MM-DD`; an offset date-time converted
/// to UTC, without fractions of a second, as `YYYY-MM-DDTHH:MM:SSZ`; anything else is an error.
fn added_text(file: &Path, path: &str, added: &toml::value::Datetime) -> Result<String> {
    use toml::value::Offset;
    let error = || Error::Config {
        path: file.to_path_buf(),
        message: format!(
            "{path}: `added` is a date (2026-11-24) or a date-time with its offset (2026-11-24T18:48:18Z)"
        ),
    };
    match (added.date, added.time, added.offset) {
        (Some(d), None, None) => Ok(format!("{:04}-{:02}-{:02}", d.year, d.month, d.day)),
        (Some(d), Some(t), Some(offset)) => {
            let minutes = match offset {
                Offset::Z => 0,
                Offset::Custom { minutes } => i64::from(minutes),
            };
            let days = days_from_civil(i64::from(d.year), i64::from(d.month), i64::from(d.day));
            let seconds = days * 86_400 + i64::from(t.hour) * 3600 + i64::from(t.minute) * 60 + i64::from(t.second)
                - minutes * 60;
            Ok(utc_text(seconds))
        }
        _ => Err(error()),
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Seconds since 1970-01-01T00:00:00Z as `YYYY-MM-DDTHH:MM:SSZ` (Howard Hinnant's `civil_from_days`).
pub(crate) fn utc_text(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
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
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> Environment {
        Environment::from_map(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<HashMap<_, _>>())
    }

    /// The Hugging Face cache: the first default location.
    fn hub(l: &Locations) -> PathBuf {
        l.read.iter().find(|l| l.what.starts_with("the Hugging Face cache")).unwrap().path.clone()
    }

    #[test]
    fn the_hugging_face_cache_is_found_as_huggingface_hub_finds_it() {
        let l = Locations::load_from(env(&[("HOME", "/h")])).unwrap();
        assert_eq!(hub(&l), PathBuf::from("/h/.cache/huggingface/hub"));
        let l = Locations::load_from(env(&[("HOME", "/h"), ("XDG_CACHE_HOME", "/x")])).unwrap();
        assert_eq!(hub(&l), PathBuf::from("/x/huggingface/hub"));
        let l = Locations::load_from(env(&[("HOME", "/h"), ("HF_HOME", "/hf"), ("HUGGINGFACE_HUB_CACHE", "/old")]))
            .unwrap();
        assert_eq!(hub(&l), PathBuf::from("/old"));
        let l =
            Locations::load_from(env(&[("HOME", "/h"), ("HF_HUB_CACHE", "/new"), ("HUGGINGFACE_HUB_CACHE", "/old")]))
                .unwrap();
        assert_eq!(hub(&l), PathBuf::from("/new"));
        assert_eq!(hf_home(&env(&[("HOME", "/h")])), PathBuf::from("/h/.cache/huggingface"));
    }

    #[test]
    fn layers_add_locations_and_the_environment_names_the_folders_written() {
        let dir = test_dir("layers");
        let config = dir.join("config.toml");
        std::fs::write(&config, "[[location]]\npath = \"/data\"\n[[location]]\npath = \"/o\"\nkind = \"ollama\"\n[names]\nq = \"Qwen/Qwen3-0.6B-GGUF:Q8_0\"\n").unwrap();
        let cfg = config.to_str().unwrap();
        let l =
            Locations::load_from(env(&[("HOME", "/h"), ("TENSTAG_CONFIG", cfg), ("TENSTAG_PATH", "/p1:/p2")])).unwrap();
        let paths: Vec<_> = l.read.iter().map(|l| l.path.to_str().unwrap()).collect();
        assert_eq!(&paths[..5], ["/data", "/o", "/p1", "/p2", "/h/.cache/huggingface/hub"]);
        assert_eq!(l.read[1].kind, Kind::Ollama);
        assert_eq!(l.names["q"], "Qwen/Qwen3-0.6B-GGUF:Q8_0");
        if cfg!(target_os = "linux") {
            assert_eq!(l.cache, Path::new("/h/.cache/tenstag"));
            let l = Locations::load_from(env(&[
                ("HOME", "/h"),
                ("XDG_CACHE_HOME", "relative"),
                ("TENSTAG_CACHE_DIR", "/c"),
            ]))
            .unwrap();
            assert_eq!(l.cache, Path::new("/c"));
        }

        std::fs::write(&config, "defaults = false\n").unwrap();
        let l = Locations::load_from(env(&[("HOME", "/h"), ("TENSTAG_CONFIG", cfg)])).unwrap();
        assert!(l.read.is_empty());
        std::fs::write(&config, "download = \"~/dl\"\n").unwrap();
        assert!(Locations::load_from(env(&[("TENSTAG_CONFIG", cfg)])).is_err(), "downloads are gone");
        std::fs::write(&config, "colour = 1\n").unwrap();
        assert!(matches!(Locations::load_from(env(&[("TENSTAG_CONFIG", cfg)])), Err(Error::Config { .. })));
    }

    #[test]
    fn a_walked_location_takes_its_depth_and_smallest_file_from_the_configuration() {
        let dir = test_dir("depths");
        let config = dir.join("config.toml");
        let c = config.to_str().unwrap();
        std::fs::write(&config, "defaults = false\n[[location]]\npath = \"/deep\"\ndepth = 12\nmin_size = 0\n[[location]]\npath = \"/plain\"\n").unwrap();
        let l = Locations::load_from(env(&[("HOME", "/h"), ("TENSTAG_CONFIG", c)])).unwrap();
        let find = |p: &str| l.read.iter().find(|x| x.path == Path::new(p)).unwrap().clone();
        assert_eq!((find("/deep").depth, find("/deep").min_size), (Some(12), 0));
        assert_eq!((find("/plain").depth, find("/plain").min_size), (Some(DEFAULT_DEPTH), DEFAULT_MIN_SIZE));

        // `class` is no longer a key: an error that says so, not a silent change.
        std::fs::write(&config, "[[location]]\npath = \"/comfy\"\nclass = \"diffusion\"\n").unwrap();
        match Locations::load_from(env(&[("HOME", "/h"), ("TENSTAG_CONFIG", c)])) {
            Err(Error::Config { message, .. }) => assert!(message.contains("removed"), "{message}"),
            other => panic!("{other:?}"),
        }

        // A store read by its own layout has no depth: setting one is an error, not ignored.
        std::fs::write(&config, "[[location]]\npath = \"/o\"\nkind = \"ollama\"\ndepth = 3\n").unwrap();
        assert!(matches!(
            Locations::load_from(env(&[("HOME", "/h"), ("TENSTAG_CONFIG", c)])),
            Err(Error::Config { .. })
        ));
    }

    #[test]
    fn gpt4all_and_modelscope_are_found_as_their_documents_say() {
        let home = test_dir("g4a-home");
        let h = home.to_str().unwrap();
        let find = |l: &Locations, kind: Kind| l.read.iter().find(|x| x.kind == kind).unwrap().path.clone();
        let l = Locations::load_from(env(&[("HOME", h), ("USERPROFILE", h)])).unwrap();
        assert_eq!(find(&l, Kind::ModelScope), home.join(".cache/modelscope"));
        if cfg!(target_os = "linux") {
            assert_eq!(find(&l, Kind::Gpt4All), home.join(".local/share/nomic.ai/GPT4All"));
            // G4A-001: modelPath in [General] moves it; an escaped value is not decoded.
            std::fs::create_dir_all(home.join(".config/nomic.ai")).unwrap();
            let ini = home.join(".config/nomic.ai/GPT4All.ini");
            std::fs::write(&ini, "[General]\nmodelPath=/data/gpt4all/\n[download]\nmodelPath=/no\n").unwrap();
            let l = Locations::load_from(env(&[("HOME", h)])).unwrap();
            assert_eq!(find(&l, Kind::Gpt4All), PathBuf::from("/data/gpt4all/"));
            std::fs::write(&ini, "[General]\nmodelPath=\"/data/\\x4e00/\"\n").unwrap();
            let l = Locations::load_from(env(&[("HOME", h)])).unwrap();
            assert_eq!(find(&l, Kind::Gpt4All), home.join(".local/share/nomic.ai/GPT4All"));
        }
        let l = Locations::load_from(env(&[("HOME", h), ("MS_CACHE_HOME", "/ms")])).unwrap();
        assert_eq!(find(&l, Kind::ModelScope), PathBuf::from("/ms"));
    }

    #[test]
    fn jan_docker_and_ramalama_are_found_as_their_documents_say() {
        let home = test_dir("jdr-home");
        let h = home.to_str().unwrap();
        let find = |l: &Locations, kind: Kind| l.read.iter().find(|x| x.kind == kind).unwrap().path.clone();
        let l = Locations::load_from(env(&[("HOME", h), ("USERPROFILE", h)])).unwrap();
        assert_eq!(find(&l, Kind::DockerModels), home.join(".docker/models"));
        assert_eq!(find(&l, Kind::RamaLama), home.join(".local/share/ramalama"));
        if cfg!(target_os = "linux") {
            assert_eq!(find(&l, Kind::Jan), home.join(".local/share/Jan/data"));
            // JAN-001: data_folder in settings.json moves it.
            std::fs::create_dir_all(home.join(".local/share/Jan")).unwrap();
            std::fs::write(home.join(".local/share/Jan/settings.json"), r#"{"data_folder": "/data/jan"}"#).unwrap();
            // RML-001: store in [ramalama] of a ramalama.conf.
            std::fs::create_dir_all(home.join(".config/ramalama/ramalama.conf.d")).unwrap();
            std::fs::write(home.join(".config/ramalama/ramalama.conf"), "[ramalama]\nstore = \"/data/one\"\n").unwrap();
            std::fs::write(home.join(".config/ramalama/ramalama.conf.d/b.conf"), "[ramalama]\nstore = \"/data/rl\"\n")
                .unwrap();
            let l = Locations::load_from(env(&[("HOME", h)])).unwrap();
            assert_eq!(find(&l, Kind::Jan), PathBuf::from("/data/jan"));
            assert_eq!(find(&l, Kind::RamaLama), PathBuf::from("/data/rl"), "a later file wins");
        }
        let l = Locations::load_from(env(&[("HOME", h), ("MODELS_PATH", "/m"), ("RAMALAMA_STORE", "/r")])).unwrap();
        assert_eq!(
            (find(&l, Kind::DockerModels), find(&l, Kind::RamaLama)),
            (PathBuf::from("/m"), PathBuf::from("/r"))
        );
    }
}

#[cfg(test)]
mod store_tests {
    use super::*;
    use crate::test_dir;
    use std::collections::HashMap;

    fn load(pairs: &[(&str, &str)]) -> Locations {
        let vars: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Locations::load_from(Environment::from_map(vars)).unwrap()
    }

    fn place(l: &Locations, kind: Kind, what: &str) -> Option<(PathBuf, String)> {
        l.read.iter().find(|x| x.kind == kind && x.what.contains(what)).map(|x| (x.path.clone(), x.what.clone()))
    }

    #[test]
    fn lm_studio_is_found_through_its_pointer_and_its_settings() {
        let home = test_dir("lms-home");
        let h = home.to_str().unwrap();
        // LMS-001: no pointer, no ~/.cache/lm-studio: ~/.lmstudio; LMS-002: its models folder.
        let l = load(&[("HOME", h)]);
        assert_eq!(place(&l, Kind::LmStudio, "LM Studio").unwrap().0, home.join(".lmstudio/models"));
        // An older install under ~/.cache/lm-studio keeps it.
        std::fs::create_dir_all(home.join(".cache/lm-studio")).unwrap();
        assert_eq!(place(&load(&[("HOME", h)]), Kind::LmStudio, "LM").unwrap().0, home.join(".cache/lm-studio/models"));
        // The pointer file wins, and settings.json's downloadsFolder moves the models.
        let studio = home.join("elsewhere");
        std::fs::create_dir_all(&studio).unwrap();
        std::fs::write(home.join(".lmstudio-home-pointer"), format!("{}\n", studio.display())).unwrap();
        assert_eq!(place(&load(&[("HOME", h)]), Kind::LmStudio, "LM").unwrap().0, studio.join("models"));
        std::fs::write(studio.join("settings.json"), r#"{"downloadsFolder":"/big/models"}"#).unwrap();
        let (path, what) = place(&load(&[("HOME", h)]), Kind::LmStudio, "LM").unwrap();
        assert_eq!((path, what.contains("downloadsFolder")), (PathBuf::from("/big/models"), true));
    }

    #[test]
    fn ollama_and_llama_cpp_are_found_as_their_documents_say() {
        // OLM-001: surrounding spaces and quotes removed.
        let l = load(&[("HOME", "/h"), ("OLLAMA_MODELS", " \"/data/ollama\" ")]);
        assert_eq!(place(&l, Kind::Ollama, "OLLAMA_MODELS").unwrap().0, PathBuf::from("/data/ollama"));
        let l = load(&[("HOME", "/h")]);
        assert_eq!(place(&l, Kind::Ollama, "Ollama").unwrap().0, PathBuf::from("/h/.ollama/models"));
        // LCC-001: LLAMA_CACHE as it is, else the cache directory's llama.cpp.
        let l = load(&[("HOME", "/h"), ("LLAMA_CACHE", "/lc")]);
        assert_eq!(place(&l, Kind::Directory, "llama.cpp").unwrap().0, PathBuf::from("/lc"));
        if cfg!(target_os = "linux") {
            let l = load(&[("HOME", "/h"), ("XDG_CACHE_HOME", "/x")]);
            assert_eq!(place(&l, Kind::Directory, "llama.cpp").unwrap().0, PathBuf::from("/x/llama.cpp"));
        }
    }
}
