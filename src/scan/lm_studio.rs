//! # LM Studio's models folder
//!
//! LM Studio keeps models as `<models folder>/<publisher>/<model>/<file>.gguf`, three levels down; the folder is
//! found by `locations` (specs/stores/lm-studio.md LMS-001 and LMS-002).
//!
//! Spec: specs/stores/lm-studio.md LMS-003 (IMPL, the layout) and LMS-004 (SHOULD, SUG-042): GGUF files exactly three
//! levels down are listed, as `<publisher>/<model>/<file>`, without taking `<publisher>/<model>` to be a Hugging
//! Face repository; a projector is a side file, not a model: tenstag reads no model data, so it tells a projector
//! by its name alone (a name starting with `mmproj`, in any case), as LMS-004 allows; the `general.type` test is for
//! the programs that read models. Nothing else is listed.

use std::path::Path;

use super::{Entry, gguf_entry, relative};

/// The GGUF models three levels below the models folder `root`.
pub(super) fn scan(root: &Path, found: &mut Vec<Entry>) {
    let dirs = |dir: &Path| -> Vec<std::path::PathBuf> {
        let mut out: Vec<_> =
            std::fs::read_dir(dir).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
        out.sort();
        out
    };
    for publisher in dirs(root).into_iter().filter(|p| p.is_dir()) {
        for model in dirs(&publisher).into_iter().filter(|p| p.is_dir()) {
            let projector = |p: &std::path::PathBuf| {
                p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with("mmproj"))
            };
            for file in dirs(&model).into_iter().filter(|p| p.is_file() && !projector(p)) {
                let id = relative(root, &file).trim_end_matches(".gguf").to_string();
                found.extend(gguf_entry(id, &file));
            }
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
    fn models_are_three_levels_down_and_projectors_are_not_models() {
        let root = test_dir("lm-studio");
        write(&root.join("lmstudio-community/Qwen3-0.6B-GGUF/Qwen3-0.6B-Q8_0.gguf"), b"12");
        write(&root.join("lmstudio-community/Qwen3-0.6B-GGUF/mmproj-Qwen3-0.6B-F16.gguf"), b"1");
        write(&root.join("lmstudio-community/Qwen3-0.6B-GGUF/MMPROJ-Qwen3-Q8_0.gguf"), b"1");
        write(&root.join("loose-Q8_0.gguf"), b"1");
        write(&root.join("a/b/c/deep-Q8_0.gguf"), b"1");
        let entries = scan(&[location(&root, Kind::LmStudio)]);
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["lmstudio-community/Qwen3-0.6B-GGUF/Qwen3-0.6B-Q8_0"]);
    }
}
