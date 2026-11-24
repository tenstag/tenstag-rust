//! # The environment
//!
//! tenstag reads environment variables (where the caches and folders are) and the home
//! directory. They are read through [`Environment`], so that tests and programs can give their own instead of
//! the process's.

use std::collections::HashMap;
use std::path::PathBuf;

/// Environment variables and the home directory, from the process or from a map.
#[derive(Clone, Debug, Default)]
pub struct Environment {
    /// `None`: the process's environment.
    vars: Option<HashMap<String, String>>,
}

impl Environment {
    /// The process's environment variables.
    pub fn from_process() -> Environment {
        Environment { vars: None }
    }

    /// Exactly these variables, and no others; the home directory is the `HOME` entry.
    pub fn from_map(vars: HashMap<String, String>) -> Environment {
        Environment { vars: Some(vars) }
    }

    /// A variable's value, or `None` when it is unset or empty (an empty value counts as unset, as
    /// huggingface_hub and llama.cpp treat it).
    pub fn var(&self, name: &str) -> Option<String> {
        let value = match &self.vars {
            Some(vars) => vars.get(name).cloned(),
            None => std::env::var(name).ok(),
        };
        value.filter(|v| !v.is_empty())
    }

    /// A variable's value as a path.
    pub fn path(&self, name: &str) -> Option<PathBuf> {
        self.var(name).map(PathBuf::from)
    }

    /// The home directory: `HOME` (`USERPROFILE` on Windows).
    pub fn home(&self) -> Option<PathBuf> {
        self.path(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
    }

    /// Whether a variable is set to a true value: `1`, `ON`, `YES` or `TRUE`, in any case.
    ///
    /// SPEC tenstag specs/stores/hf-hub-cache.md HFC-008 (IMPL) says "a true value"; which values are true is UPSTREAM
    /// huggingface_hub 0.36.2 constants.py `_is_true` (request 20261122-01 asks the spec to list them).
    pub fn flag(&self, name: &str) -> bool {
        self.var(name).is_some_and(|v| ["1", "ON", "YES", "TRUE"].contains(&v.to_ascii_uppercase().as_str()))
    }

    /// `~` and `$VAR` / `${VAR}` expanded, as Python's `os.path.expanduser(os.path.expandvars(...))` does for
    /// huggingface_hub's directories (SPEC tenstag specs/stores/hf-hub-cache.md HFC-001): a variable that is not set is
    /// left as written.
    pub fn expand_all(&self, text: &str) -> PathBuf {
        let mut out = String::new();
        let mut rest = text;
        while let Some(at) = rest.find('$') {
            out.push_str(&rest[..at]);
            let after = &rest[at + 1..];
            let (name, used) = if let Some(braced) = after.strip_prefix('{') {
                match braced.find('}') {
                    Some(end) => (&braced[..end], end + 2),
                    None => ("", 0),
                }
            } else {
                let end = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(after.len());
                (&after[..end], end)
            };
            match self.var(name).filter(|_| !name.is_empty()) {
                Some(value) => out.push_str(&value),
                None => out.push_str(&rest[at..at + 1 + used]),
            }
            rest = &rest[at + 1 + used..];
        }
        out.push_str(rest);
        self.expand(&out)
    }

    /// `~/...` with the home directory put in; other paths as they are.
    pub fn expand(&self, path: &str) -> PathBuf {
        match (path.strip_prefix("~/"), self.home()) {
            (Some(rest), Some(home)) => home.join(rest),
            _ if path == "~" => self.home().unwrap_or_else(|| PathBuf::from(path)),
            _ => PathBuf::from(path),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_and_home_are_expanded_as_python_expands_them() {
        let env = Environment::from_map(
            [("HOME", "/h"), ("A", "/a"), ("B_2", "b")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        );
        assert_eq!(env.expand_all("$A/x/${B_2}"), PathBuf::from("/a/x/b"));
        assert_eq!(env.expand_all("~/c/$UNSET/${NONE}/$"), PathBuf::from("/h/c/$UNSET/${NONE}/$"));
        assert_eq!(env.expand_all("${A"), PathBuf::from("${A"));
    }
}
