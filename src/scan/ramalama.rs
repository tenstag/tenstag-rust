//! # RamaLama's store
//!
//! RamaLama keeps each model it pulled in a folder per transport, organisation and name, with blobs, snapshots and a
//! ref file per tag naming the snapshot's files and their types:
//!
//! ```text
//!   <store>/store/huggingface/Qwen/Qwen3-0.6B-GGUF/refs/latest.json     {"hash": "<snapshot>", "files": [{name,
//type}]} !   <store>/store/huggingface/Qwen/Qwen3-0.6B-GGUF/snapshots/<snapshot>/Qwen3-0.6B-Q8_0.gguf     a link to a
//blob !   <store>/store/huggingface/Qwen/Qwen3-0.6B-GGUF/blobs/sha256-<hex>    the files; `.partial` while downloading
//! ```
//!
//! Entries are named as RamaLama lists them: `<transport>://<organisation>/<name>:<tag>`. Spec: tenstag
//! specs/stores/ramalama.md RML-001 to RML-005 (SUG-048). The store folder is found in locations.rs.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Entry, Format, file_name, new_entry};
use crate::files::GgufName;

/// The models of RamaLama's store folder `store`.
///
/// SPEC tenstag specs/stores/ramalama.md RML-002 (IMPL): `store/<transport>/<organisation>/<name>/` with `blobs/`,
/// `refs/` and `snapshots/` (the organisation is the name itself when there is none, so such a folder is one level less
/// deep). RML-003 (IMPL): a ref file's `files` of type `gguf` are the model, in order; an older text ref file is the
/// snapshot hash, then a file per line. RML-004 (IMPL): a file missing from its snapshot means the model is partial.
/// RML-005 (SHOULD, SUG-048): one model per tag, named `<transport>://<organisation>/<name>:<tag>`; a partial model is
/// not listed; `repos/` is not part of the store. Nothing is written.
///
/// DECISION: in an older text ref file, whose lines carry no types, the `.gguf` files not named `mmproj*` are the
/// model. A model with no GGUF file (safetensors) is listed as [`Format::Other`].
pub(super) fn scan(store: &Path, found: &mut Vec<Entry>) {
    for transport in folders(&store.join("store")) {
        for first in folders(&transport) {
            if is_model(&first) {
                model(&file_name(&transport), &file_name(&first), &first, found);
                continue;
            }
            for second in folders(&first).into_iter().filter(|f| is_model(f)) {
                let name = format!("{}/{}", file_name(&first), file_name(&second));
                model(&file_name(&transport), &name, &second, found);
            }
        }
    }
}

fn is_model(dir: &Path) -> bool {
    ["refs", "blobs", "snapshots"].iter().all(|d| dir.join(d).is_dir())
}

fn folders(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = read
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && !file_name(p).starts_with('.'))
        .collect();
    out.sort();
    out
}

/// The models, one per tag, of the model folder `dir`, named `name` under `transport`.
fn model(transport: &str, name: &str, dir: &Path, found: &mut Vec<Entry>) {
    let Ok(read) = std::fs::read_dir(dir.join("refs")) else { return };
    let mut refs: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
    // By tag, a tag's JSON file before its text file (`latest.json` before `latest`, which a plain sort reverses).
    let key = |p: &PathBuf| {
        let file = file_name(p);
        let json = file.ends_with(".json");
        (file.strip_suffix(".json").unwrap_or(&file).to_string(), !json)
    };
    refs.sort_by_key(key);
    let mut tags = std::collections::HashSet::new();
    for path in refs {
        let file = file_name(&path);
        let tag = file.strip_suffix(".json").unwrap_or(&file).to_string();
        let Some((snapshot, files)) = read_ref(&path) else { continue };
        // A tag with both forms is read once, from its JSON file (sorted first). SPEC tenstag specs/listing.md LST-013.
        if !tags.insert(tag.clone()) {
            continue;
        }
        let folder = dir.join("snapshots").join(&snapshot);
        if files.iter().any(|(f, _)| !folder.join(f).is_file()) {
            continue;
        }
        let parts: Vec<PathBuf> = files.iter().filter(|(_, model)| *model).map(|(f, _)| folder.join(f)).collect();
        let id = format!("{transport}://{name}:{tag}");
        let mut entry = match parts.first() {
            Some(first) => {
                let mut entry = new_entry(id, first.clone(), Format::Gguf);
                entry.quant = GgufName::parse(&file_name(first)).map(|n| n.tag).unwrap_or_default();
                entry
            }
            None => {
                let mut entry = new_entry(id, folder.clone(), Format::Other);
                entry.note = "a model with no GGUF file (safetensors), in RamaLama's store".into();
                entry
            }
        };
        entry.bytes = parts.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
        if parts.len() > 1 {
            entry.parts = parts;
        }
        found.push(entry);
    }
}

/// A ref file's snapshot and its files, each with whether it is part of the model (a GGUF file).
fn read_ref(path: &Path) -> Option<(String, Vec<(String, bool)>)> {
    let text = std::fs::read_to_string(path).ok()?;
    if path.extension().is_some_and(|e| e == "json") {
        let value: Value = serde_json::from_str(&text).ok()?;
        let files = value["files"]
            .as_array()?
            .iter()
            .filter_map(|f| Some((f["name"].as_str()?.to_string(), f["type"].as_str() == Some("gguf"))))
            .collect();
        return Some((value["hash"].as_str()?.to_string(), files));
    }
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let snapshot = lines.next()?.to_string();
    let files = lines.map(|f| (f.to_string(), f.ends_with(".gguf") && !f.starts_with("mmproj"))).collect();
    Some((snapshot, files))
}

#[cfg(test)]
mod tests {
    use crate::locations::Kind;
    use crate::scan::scan;
    use crate::scan::tests::{location, write};
    use crate::test_dir;

    #[test]
    fn one_model_per_tag_named_as_ramalama_names_it() {
        let store = test_dir("ramalama");
        let m = store.join("store/huggingface/Qwen/Qwen3-0.6B-GGUF");
        write(&m.join("blobs/sha256-aa"), b"123");
        write(&m.join("snapshots/s1/Qwen3-0.6B-Q8_0.gguf"), b"123");
        write(&m.join("snapshots/s1/mmproj-F16.gguf"), b"1");
        write(
            &m.join("refs/latest.json"),
            br#"{"version": "v1.0.1", "hash": "s1", "files": [
                {"hash": "sha256:aa", "name": "Qwen3-0.6B-Q8_0.gguf", "type": "gguf"},
                {"hash": "sha256:bb", "name": "mmproj-F16.gguf", "type": "mmproj"}]}"#,
        );
        write(&m.join("refs/old"), b"s1\nQwen3-0.6B-Q8_0.gguf\nmmproj-F16.gguf\n");
        write(&m.join("refs/partial.json"), br#"{"hash": "s2", "files": [{"name": "x.gguf", "type": "gguf"}]}"#);
        let o = store.join("store/ollama/smollm");
        write(&o.join("blobs/x"), b"1");
        write(&o.join("snapshots/s/smollm.gguf"), b"12");
        write(&o.join("refs/latest.json"), br#"{"hash": "s", "files": [{"name": "smollm.gguf", "type": "gguf"}]}"#);

        let entries = scan(&[location(&store, Kind::RamaLama)]);
        let found: Vec<(&str, &str, u64)> =
            entries.iter().map(|e| (e.id.as_str(), e.quant.as_str(), e.bytes)).collect();
        assert_eq!(
            found,
            [
                ("huggingface://Qwen/Qwen3-0.6B-GGUF:latest", "Q8_0", 3),
                ("huggingface://Qwen/Qwen3-0.6B-GGUF:old", "Q8_0", 3),
                ("ollama://smollm:latest", "", 2),
            ],
            "the partial model is not listed; the projector is not the model"
        );
    }
}
