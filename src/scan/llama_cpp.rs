//! # llama.cpp's old flat cache: names from its manifests
//!
//! Before 2026-03-24 llama.cpp stored each file of an `-hf` download flat in its cache directory as
//! `<owner>_<repo>_<path>` (every `/` and `\` of the path made `_`), and kept, per repository and tag, the Hub's
//! manifest answer as `manifest=<owner>=<repo>=<tag>.json`, whose `ggufFile.rfilename` names the model file. Such
//! directories remain where llama.cpp did not migrate them (it stopped migrating on 2026-05-18).
//!
//! Spec: specs/stores/llama-cpp-cache.md LCC-001 (the directory, `locations`), LCC-003 (IMPL, the layout), LCC-004
//! (IMPL), LCC-005 (SHOULD, SUG-040): a flat file is named `<owner>/<repo>:<tag>` only through a manifest; without
//! one it is listed by its file name, as the files llama.cpp still writes there are (LCC-002).

use std::collections::HashMap;
use std::path::Path;

/// For each flat file in `dir` that a manifest names, the id `<owner>/<repo>:<tag>` (SUG-040). The projector
/// (`mmprojFile`) is a side file, not a model, and gets no id.
pub(super) fn manifest_names(dir: &Path) -> HashMap<String, String> {
    let mut names = HashMap::new();
    let Ok(read) = std::fs::read_dir(dir) else { return names };
    for e in read.filter_map(|e| e.ok()) {
        let file = e.file_name().to_string_lossy().to_string();
        let Some(inner) = file.strip_prefix("manifest=").and_then(|f| f.strip_suffix(".json")) else { continue };
        let parts: Vec<&str> = inner.splitn(3, '=').collect();
        let [owner, repo, tag] = parts[..] else { continue };
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let Some(path) = manifest["ggufFile"]["rfilename"].as_str() else { continue };
        let flat = format!("{owner}_{repo}_{}", path.replace(['/', '\\'], "_"));
        if dir.join(&flat).is_file() {
            names.insert(flat, format!("{owner}/{repo}:{tag}"));
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use crate::locations::Kind;
    use crate::scan::scan;
    use crate::scan::tests::{location, write};
    use crate::test_dir;

    #[test]
    fn old_flat_files_are_named_through_their_manifests_only() {
        let root = test_dir("llama-cpp");
        write(&root.join("ggml-org_gemma-3-1b-it-GGUF_gemma-3-1b-it-Q4_K_M.gguf"), b"123");
        let manifest = br#"{"ggufFile":{"rfilename":"gemma-3-1b-it-Q4_K_M.gguf","lfs":{"sha256":"00"}}}"#;
        write(&root.join("manifest=ggml-org=gemma-3-1b-it-GGUF=latest.json"), manifest);
        write(&root.join("some_user_repo_x-Q8_0.gguf"), b"12");
        write(&root.join("model-from-url.gguf"), b"1");
        let entries = scan(&[location(&root, Kind::Directory)]);
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["ggml-org/gemma-3-1b-it-GGUF:latest", "model-from-url", "some_user_repo_x-Q8_0"]);
    }
}
