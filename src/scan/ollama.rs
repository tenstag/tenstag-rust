//! # Ollama's store
//!
//! Ollama names a model `[host/][namespace/]model[:tag]` (defaults registry.ollama.ai, library, latest) and keeps,
//! for each, a manifest that lists its layers, each a blob named by its SHA-256:
//!
//! ```text
//!   manifests-v2/<host>/<namespace>/<model>/<tag>    a link to the manifest's blob (read first; the public host is
//!                                                    written ollama.com, ':' in a host %3A)
//!   manifests/<host>/<namespace>/<model>/<tag>       the legacy manifest file (read when v2 has no entry)
//!   blobs/sha256-<64 hex>                            the layers, the config and the manifests themselves
//! ```
//!
//! A manifest may be a list of manifests, one per runner and format. The config blob's `model_format` says whether
//! the weights are GGUF (`gguf`, or empty in older models): then the `image.model` layers are the GGUF file, or the
//! parts of a split one in order. A `safetensors` model has no GGUF file and is listed as [`Format::Other`].
//!
//! Spec: specs/stores/ollama.md OLM-001 (the directory, `locations`), OLM-002 to OLM-007 (IMPL), OLM-008 (SHOULD,
//! SUG-041: a manifest list is resolved to its gguf entry; model layers only when model_format is gguf or empty;
//! each blob's GGUF magic is checked; other formats are listed as not GGUF).

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Entry, Format, new_entry};

/// The public host, as the legacy tree writes it; manifests-v2 writes it `ollama.com` (OLM-002, OLM-003).
const PUBLIC: &str = "registry.ollama.ai";

/// The media types read (OLM-005, OLM-006).
const LIST: &str = "application/vnd.ollama.manifest.list.v2+json";
const MODEL: &str = "application/vnd.ollama.image.model";
const TENSOR: &str = "application/vnd.ollama.image.tensor";

/// The models of the Ollama store at `root`.
pub(super) fn scan(root: &Path, found: &mut Vec<Entry>) {
    let mut seen = HashSet::new();
    for tree in ["manifests-v2", "manifests"] {
        let mut names = Vec::new();
        list_four_deep(&root.join(tree), &mut Vec::new(), &mut names);
        names.sort();
        for (parts, path) in names {
            let [host, namespace, model, tag] = &parts[..] else { continue };
            let host = host.replace("%3A", ":");
            let host = if host == "ollama.com" { PUBLIC.to_string() } else { host };
            let name = display(&host, namespace, model, tag);
            // OLM-002: names compare without regard to case; OLM-003: manifests-v2 first.
            if !seen.insert(name.to_lowercase()) {
                continue;
            }
            let Some(manifest) = read_json(&path).and_then(|m| resolve_list(root, m)) else { continue };
            found.extend(entry(root, &name, &manifest));
        }
    }
}

/// `ollama:<name>` as Ollama would show it: the defaults left out.
fn display(host: &str, namespace: &str, model: &str, tag: &str) -> String {
    match (host, namespace) {
        (PUBLIC, "library") => format!("ollama:{model}:{tag}"),
        (PUBLIC, _) => format!("ollama:{namespace}/{model}:{tag}"),
        _ => format!("ollama:{host}/{namespace}/{model}:{tag}"),
    }
}

/// The files four directories below `dir` (host, namespace, model, tag), with those four names.
fn list_four_deep(dir: &Path, prefix: &mut Vec<String>, out: &mut Vec<(Vec<String>, PathBuf)>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.filter_map(|e| e.ok()) {
        prefix.push(e.file_name().to_string_lossy().to_string());
        let path = e.path();
        if prefix.len() == 4 {
            if path.is_file() {
                out.push((prefix.clone(), path));
            }
        } else if path.is_dir() {
            list_four_deep(&path, prefix, out);
        }
        prefix.pop();
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// A blob's path for a digest `sha256:<64 hex>`, if the digest is one and the blob is there (OLM-004).
fn blob(root: &Path, digest: &str) -> Option<PathBuf> {
    let hex = digest.strip_prefix("sha256:")?;
    let valid = hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase());
    let path = root.join("blobs").join(format!("sha256-{hex}"));
    (valid && path.is_file()).then_some(path)
}

/// A manifest list resolved to its entry of format gguf whose manifest blob is present, else to its first present
/// entry (listed as not GGUF); a single manifest as it is (OLM-005, SUG-041).
fn resolve_list(root: &Path, manifest: Value) -> Option<Value> {
    if manifest["mediaType"] != LIST {
        return Some(manifest);
    }
    let entries = manifest["manifests"].as_array()?;
    let present = |e: &&Value| e["digest"].as_str().and_then(|d| blob(root, d)).is_some();
    let chosen =
        entries.iter().filter(present).find(|e| e["format"] == "gguf").or_else(|| entries.iter().find(present))?;
    let mut single = read_json(&blob(root, chosen["digest"].as_str()?)?)?;
    if single["format"].is_null() {
        single["format"] = chosen["format"].clone();
    }
    Some(single)
}

/// Whether the file at `path` starts with the GGUF magic.
fn is_gguf(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"GGUF"
}

/// The entry of one manifest (OLM-006, OLM-007, SUG-041).
fn entry(root: &Path, name: &str, manifest: &Value) -> Option<Entry> {
    let layers = manifest["layers"].as_array()?;
    let config = manifest["config"]["digest"].as_str().and_then(|d| blob(root, d)).and_then(|p| read_json(&p));
    let config = config.unwrap_or(Value::Null);
    let format = config["model_format"].as_str().or_else(|| manifest["format"].as_str()).unwrap_or("").to_string();
    let quant = config["file_type"].as_str().unwrap_or("").to_string();
    let of = |kind: &str| -> Option<Vec<(PathBuf, u64)>> {
        layers
            .iter()
            .filter(|l| l["mediaType"] == kind)
            .map(|l| {
                let path = blob(root, l["digest"].as_str()?)?;
                let bytes = std::fs::metadata(&path).ok()?.len();
                Some((path, bytes))
            })
            .collect()
    };
    if format.is_empty() || format == "gguf" {
        let parts = of(MODEL)?;
        if parts.is_empty() || !parts.iter().all(|(p, _)| is_gguf(p)) {
            return None;
        }
        let mut entry = new_entry(name.to_string(), parts[0].0.clone(), Format::Gguf);
        entry.bytes = parts.iter().map(|(_, b)| b).sum();
        entry.quant = quant;
        if parts.len() > 1 {
            entry.parts = parts.into_iter().map(|(p, _)| p).collect();
        }
        Some(entry)
    } else {
        // SPEC tenstag specs/listing.md LST-008 (MUST): the tensor layers whose blobs are present; a missing one
        // counts nothing.
        let tensors: u64 = layers
            .iter()
            .filter(|l| l["mediaType"] == TENSOR)
            .filter_map(|l| blob(root, l["digest"].as_str()?))
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        let mut entry = new_entry(name.to_string(), root.join("blobs"), Format::Other);
        entry.bytes = tensors;
        entry.quant = quant;
        entry.note = format!("an Ollama {format} model, not a GGUF file");
        Some(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::locations::Kind;
    use crate::scan::scan;
    use crate::scan::tests::{location, write};
    use crate::test_dir;
    use sha2::{Digest, Sha256};

    /// Writes `bytes` as a blob; returns its digest.
    fn put(root: &Path, bytes: &[u8]) -> String {
        let hex: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
        write(&root.join("blobs").join(format!("sha256-{hex}")), bytes);
        format!("sha256:{hex}")
    }

    fn manifest(config: &str, layers: &[(&str, &str)]) -> String {
        let layers: Vec<String> =
            layers.iter().map(|(t, d)| format!(r#"{{"mediaType":"{t}","digest":"{d}","size":1}}"#)).collect();
        format!(
            r#"{{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"{config}"}},"layers":[{}]}}"#,
            layers.join(",")
        )
    }

    #[test]
    fn manifests_v2_legacy_lists_splits_and_safetensors_are_read_as_the_spec_says() {
        let root = test_dir("ollama");
        let gguf = put(&root, b"GGUF-one");
        let (part1, part2) = (put(&root, b"GGUF-part-1"), put(&root, b"GGUF-part-2"));
        let not_gguf = put(&root, b"nope");
        let tensor = put(&root, b"safetensors-bytes");
        let gguf_config = put(&root, br#"{"model_format":"gguf","file_type":"Q4_K_M"}"#);
        let old_config = put(&root, br#"{"file_type":"Q8_0"}"#);
        let st_config = put(&root, br#"{"model_format":"safetensors","file_type":"BF16"}"#);

        // v2: qwen3:0.6b (one GGUF), a split model; legacy: the same qwen3:0.6b (ignored), an old model (no
        // model_format), a safetensors model, a model whose blob is not GGUF, and a manifest list.
        let v2 = root.join("manifests-v2/ollama.com/library");
        write(&v2.join("qwen3/0.6b"), manifest(&gguf_config, &[(MODEL, &gguf)]).as_bytes());
        write(&v2.join("big/latest"), manifest(&gguf_config, &[(MODEL, &part1), (MODEL, &part2)]).as_bytes());
        let legacy = root.join("manifests/registry.ollama.ai");
        write(&legacy.join("library/qwen3/0.6b"), manifest(&old_config, &[(MODEL, &gguf)]).as_bytes());
        write(&legacy.join("someone/old/latest"), manifest(&old_config, &[(MODEL, &gguf)]).as_bytes());
        write(&legacy.join("library/st/latest"), manifest(&st_config, &[(TENSOR, &tensor)]).as_bytes());
        write(&legacy.join("library/bad/latest"), manifest(&gguf_config, &[(MODEL, &not_gguf)]).as_bytes());
        let single = put(&root, manifest(&gguf_config, &[(MODEL, &gguf)]).as_bytes());
        let list = format!(
            r#"{{"mediaType":"{LIST}","manifests":[{{"digest":"sha256:{}","runner":"mlx","format":"safetensors"}},{{"digest":"{single}","runner":"llamacpp","format":"gguf"}}]}}"#,
            "0".repeat(64)
        );
        write(&root.join("manifests/hf.co/org/Repo-GGUF/Q4_K_M"), list.as_bytes());

        let entries = scan(&[location(&root, Kind::Ollama)]);
        let ids: Vec<(&str, Format, &str, usize)> =
            entries.iter().map(|e| (e.id.as_str(), e.format, e.quant.as_str(), e.parts.len())).collect();
        assert_eq!(
            ids,
            [
                ("ollama:big:latest", Format::Gguf, "Q4_K_M", 2),
                ("ollama:qwen3:0.6b", Format::Gguf, "Q4_K_M", 0),
                ("ollama:hf.co/org/Repo-GGUF:Q4_K_M", Format::Gguf, "Q4_K_M", 0),
                ("ollama:st:latest", Format::Other, "BF16", 0),
                ("ollama:someone/old:latest", Format::Gguf, "Q8_0", 0),
            ]
        );
        assert_eq!(entries[0].bytes, 22);
    }
}
