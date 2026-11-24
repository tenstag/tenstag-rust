//! # Errors
//!
//! One error type for the crate. Each message is written for the person running the program: it says what went
//! wrong and, where there is one, what to do about it.

use std::path::PathBuf;

/// What can go wrong while finding, naming or tagging a model.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file or directory could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// What the operating system said.
        source: std::io::Error,
    },
    /// The configuration file could not be understood.
    #[error("the configuration file {}: {message}", path.display())]
    Config {
        /// The file.
        path: PathBuf,
        /// What is wrong with it.
        message: String,
    },
    /// No model has this name, in any location.
    #[error("no model found for {name:?}{hint}")]
    NotFound {
        /// The name given.
        name: String,
        /// More to say (what was searched), starting with ": " or empty.
        hint: String,
    },
    /// More than one model matches a partial name.
    #[error("{name:?} matches {} models; give more of the name:\n  {}", candidates.len(), candidates.join("\n  "))]
    Ambiguous {
        /// The name given.
        name: String,
        /// The ids of the models it matches.
        candidates: Vec<String>,
    },
    /// A tag that is not one, or a model that cannot be tagged (tenstag specs/tags.md).
    #[error("tags: {0}")]
    Tag(String),
}

impl Error {
    /// An [`Error::Io`] for `path`.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Error {
        Error::Io { path: path.into(), source }
    }
}

/// The crate's result type.
pub type Result<T> = std::result::Result<T, Error>;
