//! # The ModelScope cache
//!
//! ModelScope's Python libraries keep downloaded repositories in their own cache, in a current and an older layout:
//!
//! ```text
//!   <root>/models/<owner>--<name>/snapshots/<revision>/<files>      current: one folder per revision as asked for
//!   <root>/models/<owner>/<name, '.' written '___'>/<files>          older (ModelScope 1.37 and before)
//!   <root>/hub/models/<owner>/<name___>/<files>                      older, the default root of those versions
//!   <root>/models/<owner>/<name>  ->  a current snapshot             a link left for older programs: read once
//! ```
//!
//! Each folder's models are read as in the Hugging Face cache: its GGUF files (split sets by their first part, quant
//! tags by llama.cpp's rule), or the folder itself when it is a model directory, a pipeline or an adapter. Entries are
//! named `modelscope:<owner>/<name>[:TAG][@revision]`, so that a Hugging Face repository's name never resolves to a
//! ModelScope copy.
//!
//! Spec: tenstag specs/stores/modelscope-cache.md MSC-001 to MSC-007 (SUG-045). The root is found in locations.rs.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{Entry, directory_entry, file_name, gguf_entry, list_files};
use crate::files::GgufName;

/// The models in the ModelScope cache at `root`.
///
/// SPEC tenstag specs/stores/modelscope-cache.md MSC-002 (IMPL): `models/<owner>--<name>/snapshots/<revision>/`, a
/// revision as asked for. MSC-004 (IMPL): the older `models/<owner>/<name___>/` and `hub/models/<owner>/<name___>/`.
/// MSC-005 (IMPL): links from the older paths to snapshots, so each real folder is read once. MSC-006 (MUST): `.msc`
/// and `.mv` are pickles, never opened (they are not models, so not listed). MSC-007 (SHOULD, SUG-045): every revision
/// listed, `@<revision>` left out for `master`; `.incomplete`, `._____temp`, `.lock` skipped.
///
/// DECISION: the earliest layout, `<root>/<owner>/<name>` (MSC-004, ModelScope 1.40.3's fallback), is not read: any
/// folder two levels down would match it.
pub(super) fn scan(root: &Path, found: &mut Vec<Entry>) {
    let mut seen = HashSet::new();
    let models = root.join("models");
    for repo in folders(&models) {
        let name = file_name(&repo);
        if let Some((owner, model)) = name.split_once("--") {
            for snapshot in folders(&repo.join("snapshots")) {
                let revision = file_name(&snapshot);
                let at = if revision == "master" { String::new() } else { format!("@{revision}") };
                folder(&format!("{owner}/{model}"), &at, &snapshot, &mut seen, found);
            }
        }
    }
    for base in [models, root.join("hub").join("models")] {
        for owner in folders(&base).into_iter().filter(|o| !file_name(o).contains("--")) {
            for repo in folders(&owner) {
                let model = file_name(&repo).replace("___", ".");
                folder(&format!("{}/{model}", file_name(&owner)), "", &repo, &mut seen, found);
            }
        }
    }
}

/// The folders in `dir`, sorted, leaving out those named `.*` (`._____temp`, `.lock`).
fn folders(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = read
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && !file_name(p).starts_with('.'))
        .collect();
    out.sort();
    out
}

/// The models of one repository folder, `repo` (`<owner>/<name>`) at revision `at` (`@<revision>` or empty).
fn folder(repo: &str, at: &str, dir: &Path, seen: &mut HashSet<PathBuf>, found: &mut Vec<Entry>) {
    let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !seen.insert(real) {
        return;
    }
    let whole = directory_entry(format!("modelscope:{repo}{at}"), dir);
    if !dir.join("model_index.json").is_file() {
        let mut files = Vec::new();
        list_files(dir, dir, &mut files);
        files.sort();
        for file in files.iter().filter(|f| f.ends_with(".gguf")) {
            let tag = GgufName::parse(file).map(|n| n.tag).unwrap_or_default();
            let id = if tag.is_empty() {
                format!("modelscope:{repo}/{file}{at}")
            } else {
                format!("modelscope:{repo}:{tag}{at}")
            };
            let Some(mut entry) = gguf_entry(id, &dir.join(file)) else { continue };
            if found.iter().any(|e| e.id == entry.id) {
                entry.id = format!("modelscope:{repo}/{file}{at}");
            }
            found.push(entry);
        }
    }
    found.extend(whole);
}

#[cfg(test)]
mod tests {
    use crate::locations::Kind;
    use crate::scan::tests::{location, write};
    use crate::scan::{Format, scan};
    use crate::test_dir;

    #[test]
    fn current_and_older_layouts_each_folder_once_and_no_partial_files() {
        let root = test_dir("modelscope");
        let m = root.join("models");
        write(&m.join("Qwen--Qwen3-0.6B-GGUF/snapshots/master/Qwen3-0.6B-Q8_0.gguf"), b"12");
        write(&m.join("Qwen--Qwen3-0.6B-GGUF/snapshots/master/Qwen3-0.6B-F16.gguf.incomplete"), b"1");
        write(&m.join("Qwen--Qwen3-0.6B-GGUF/snapshots/v1/Qwen3-0.6B-Q8_0.gguf"), b"1");
        write(&m.join("Qwen--Qwen3-0.6B/snapshots/master/config.json"), b"{}");
        write(&m.join("Qwen--Qwen3-0.6B/snapshots/master/model.safetensors"), b"1");
        write(&root.join("hub/models/old/Mini-1___5B/config.json"), b"{}");
        write(&root.join("hub/models/old/Mini-1___5B/model.safetensors"), b"1");
        write(&root.join("hub/models/old/Mini-1___5B/.msc"), b"pickle");
        write(&m.join("._____temp/x/y/z.gguf"), b"1");
        #[cfg(unix)]
        {
            std::fs::create_dir_all(m.join("Qwen")).unwrap();
            std::os::unix::fs::symlink(m.join("Qwen--Qwen3-0.6B/snapshots/master"), m.join("Qwen/Qwen3-0.6B")).unwrap();
        }

        let entries = scan(&[location(&root, Kind::ModelScope)]);
        let mut found: Vec<(&str, Format)> = entries.iter().map(|e| (e.id.as_str(), e.format)).collect();
        found.sort_by(|a, b| a.0.cmp(b.0));
        assert_eq!(
            found,
            [
                ("modelscope:Qwen/Qwen3-0.6B", Format::HfDirectory),
                ("modelscope:Qwen/Qwen3-0.6B-GGUF:Q8_0", Format::Gguf),
                ("modelscope:Qwen/Qwen3-0.6B-GGUF:Q8_0@v1", Format::Gguf),
                ("modelscope:old/Mini-1.5B", Format::HfDirectory),
            ],
            "the link to a snapshot is not a second model; '___' reads as '.'; partial and temporary files left out"
        );
    }
}
