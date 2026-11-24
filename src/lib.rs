//! # tenstag: finding models on disk, and a cache of what programs worked out about them
//!
//! Models live in many places: the Hugging Face cache (shared by huggingface_hub, transformers and llama.cpp),
//! LM Studio's folder, Ollama's store, a program's own folders. tenstag looks in all of them, gives every model a
//! name, and turns a short or partial name into the model it means. Programs tag models with what they worked out
//! from them (`arch=qwen3`, `context=32768`), so that they read each model once: the tags are kept by the model's
//! contents, not its path, so they follow a model that is moved, renamed or copied to another store. Each program
//! owns its tags, under its own name and version: through tenstag it writes only its own, and reads anyone's. An
//! owner is a courtesy between programs, not a protection (tenstag specs/tags.md TAG-012): treat tags you read as
//! unchecked input.
//!
//! Tags are a cache (tenstag specs/tags.md TAG-008): a person may delete them at any time, and a program works out
//! again what it finds missing. Something only a person can say, such as a favourite, belongs in the program's own
//! settings.
//!
//! ```text
//!   Locations::load()          where to look: the configuration file, environment variables, defaults (locations)
//!        |
//!   Library::new(locations)    what is there (scan)
//!        |
//!   library.resolve(name)      a path, a nickname, an id, a Hugging Face repository, or a search (names)
//!        |
//!   library.tags(&entry), library.tag(&entry, "arch=qwen3"), library.tagged("arch")
//!                              the model's identity (fingerprint), and its tags (tags)
//! ```
//!
//! ```no_run
//! let mut library = tenstag::Library::open_as("com.example.myapp", 1)?; // the tag owner, and its version
//! for entry in library.entries().to_vec() {
//!     if !library.tags(&entry)?.has("arch") {
//!         let arch = "qwen3"; // the program's own reading of the model
//!         library.tag(&entry, &format!("arch={arch}"))?;
//!     }
//! }
//! let qwen3 = library.tagged("arch=qwen3")?;
//! # Ok::<(), tenstag::Error>(())
//! ```
//!
//! Nothing is downloaded and no model is changed. One folder is written: the cache folder ([`Locations::cache`]),
//! which holds the fingerprints and the tags. Models are read only to fingerprint them (at most about 33 MiB of each
//! file, once), and Ollama blobs' first four bytes (specs/stores/ollama.md SUG-041).
//!
//! ## Logging
//!
//! tenstag says what it decided through the `log` crate; nothing is printed without a logger.

mod config_write;
#[cfg(test)]
mod conformance;
mod env;
mod error;
mod files;
mod fingerprint;
mod locations;
mod names;
mod scan;
mod tags;

pub use config_write::AddedToConfig;
pub use env::Environment;
pub use error::{Error, Result};
pub use fingerprint::{Identity, fingerprint_file};
pub use locations::{DEFAULT_DEPTH, DEFAULT_MIN_SIZE, Kind, Location, Locations, Source};
pub use names::{HubRef, Library};
pub use scan::{Entry, Format, HubOrigin};
pub use tags::{OwnerTags, Tags, is_owner, parse_tag, value_set};

/// README.md's examples, compiled as doc tests so that they stay true.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct ReadmeExamples;

/// A new, empty directory for one test, under the system's temporary directory.
#[cfg(test)]
pub(crate) fn test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tenstag-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
