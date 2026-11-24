//! # GPT4All's model folder
//!
//! GPT4All keeps each model as one GGUF file, named as it was downloaded, anywhere under its model folder:
//!
//! ```text
//!   <model folder>/Meta-Llama-3-8B-Instruct.Q4_0.gguf       a model, named by its file
//!   <model folder>/mine/phi-3-mini.gguf                     a model in a subfolder
//!   <model folder>/incomplete-mistral-7b.Q4_0.gguf          a download in progress: not a model
//!   <model folder>/gpt-4o-capi.rmodel                       a remote model's settings, with an API key: never opened
//! ```
//!
//! Spec: tenstag specs/stores/gpt4all.md G4A-001 to G4A-006 (SUG-044). The folder itself is found in
//! locations.rs.

use std::path::{Path, PathBuf};

use super::{Entry, Format, file_name, new_entry, relative};
use crate::files::GgufName;

/// The models under GPT4All's model folder `root`, walked `depth` parts deep.
///
/// SPEC tenstag specs/stores/gpt4all.md G4A-003 (IMPL) and G4A-006 (SHOULD, SUG-044): every `*.gguf` file at any depth
/// whose name does not start with `incomplete` is one model, named by its file name: GPT4All has no split sets and no
/// projectors. G4A-004 (IMPL): `incomplete-<file>` is a download in progress. G4A-005 (MUST): `.rmodel` files are never
/// opened (they are not listed either). DECISION: the walk is bounded by the location's depth (8 unless the
/// configuration says), as GPT4All's own is not; each real folder is read once.
pub(super) fn scan(root: &Path, depth: usize, found: &mut Vec<Entry>) {
    let mut visited = std::collections::HashSet::new();
    walk(root, root, 0, depth, &mut visited, found);
}

fn walk(
    root: &Path,
    dir: &Path,
    level: usize,
    depth: usize,
    visited: &mut std::collections::HashSet<PathBuf>,
    found: &mut Vec<Entry>,
) {
    let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if level >= depth || !visited.insert(real) {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        let name = file_name(&path);
        if path.is_dir() {
            walk(root, &path, level + 1, depth, visited, found);
        } else if name.ends_with(".gguf") && !name.starts_with("incomplete") {
            let mut entry =
                new_entry(relative(root, &path).trim_end_matches(".gguf").to_string(), path.clone(), Format::Gguf);
            entry.quant = GgufName::parse(&name).map(|n| n.tag).unwrap_or_default();
            entry.bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
            found.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::locations::Kind;
    use crate::scan::scan;
    use crate::scan::tests::{location, write};
    use crate::test_dir;

    #[test]
    fn every_gguf_file_is_a_model_and_downloads_and_remote_models_are_not() {
        let root = test_dir("gpt4all");
        write(&root.join("Meta-Llama-3-8B-Instruct.Q4_0.gguf"), b"12");
        write(&root.join("mine/phi3.gguf"), b"1");
        write(&root.join("incomplete-mistral-7b.Q4_0.gguf"), b"1");
        write(&root.join("gpt-4o-capi.rmodel"), br#"{"apiKey": "secret"}"#);
        let entries = scan(&[location(&root, Kind::Gpt4All)]);
        let found: Vec<(&str, &str)> = entries.iter().map(|e| (e.id.as_str(), e.quant.as_str())).collect();
        assert_eq!(found, [("Meta-Llama-3-8B-Instruct.Q4_0", "Q4_0"), ("mine/phi3", "")]);
    }
}
