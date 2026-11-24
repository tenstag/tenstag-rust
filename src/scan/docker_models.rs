//! # Docker Model Runner's store
//!
//! Docker's model runner keeps models as OCI artifacts: an index of models, a manifest per model, and its layers as
//! blobs named by digest:
//!
//! ```text
//!   <root>/layout.json                     {"version": "1.0.0"}: a store of this layout
//!   <root>/models.json                     {"models": [{"id": "sha256:<hex>", "tags": ["ai/smollm2:latest"], ...}]}
//!   <root>/manifests/sha256/<hex>          the model's manifest: its layers, in order, with media types
//!   <root>/blobs/sha256/<hex>              a layer: a GGUF file (or part of a split one), a projector, a license...
//!   <root>/bundles/                        the same data unpacked to run it: not listed
//! ```
//!
//! Entries are named `docker:<tag>`, one per tag. Spec: tenstag specs/stores/docker-model-runner.md DMR-001 to
//! DMR-006 (SUG-047). The root is found in locations.rs.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Entry, Format, new_entry};

/// The media types of a GGUF layer (DMR-004).
const GGUF_LAYERS: [&str; 2] = ["application/vnd.docker.ai.gguf.v3", "application/vnd.cncf.model.weight.v1.gguf"];

/// The models of the store at `root`.
///
/// SPEC tenstag specs/stores/docker-model-runner.md DMR-002 (IMPL): `layout.json`, `models.json`, `manifests/` and
/// `blobs/` named `<algorithm>/<hex>`. DMR-004 (IMPL): the GGUF layers, in manifest order, are the model (several for a
/// split one); a model with none (safetensors) is listed as [`Format::Other`]. DMR-005 (IMPL): `bundles/` holds the
/// same data. DMR-006 (SHOULD, SUG-047): only a store whose `layout.json` says version 1.0.0; one model per
/// `models.json` entry, named by its tags; `.incomplete` blobs are not models, so a model with a missing layer is not
/// listed. Nothing is written.
pub(super) fn scan(root: &Path, found: &mut Vec<Entry>) {
    let json = |path: PathBuf| std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    if json(root.join("layout.json")).as_ref().and_then(|v| v["version"].as_str()) != Some("1.0.0") {
        return;
    }
    let Some(index) = json(root.join("models.json")) else { return };
    for model in index["models"].as_array().into_iter().flatten() {
        let Some((algorithm, hex)) = model["id"].as_str().and_then(|id| id.split_once(':')) else { continue };
        let Some(manifest) = json(root.join("manifests").join(algorithm).join(hex)) else { continue };
        let blob = |digest: &str| digest.split_once(':').map(|(a, h)| root.join("blobs").join(a).join(h));
        let layers: Vec<(&str, Option<PathBuf>)> = manifest["layers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| (l["mediaType"].as_str().unwrap_or(""), l["digest"].as_str().and_then(blob)))
            .collect();
        let parts: Vec<PathBuf> =
            layers.iter().filter(|(t, _)| GGUF_LAYERS.contains(t)).filter_map(|(_, p)| p.clone()).collect();
        // A layer not yet downloaded (still `.incomplete`) means the model is not usable yet.
        if layers.iter().any(|(_, p)| p.as_ref().is_none_or(|p| !p.is_file())) {
            continue;
        }
        let bytes: u64 = parts.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
        for tag in model["tags"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let mut entry = match parts.first() {
                Some(first) => new_entry(format!("docker:{tag}"), first.clone(), Format::Gguf),
                None => {
                    let mut entry = new_entry(format!("docker:{tag}"), root.join("blobs"), Format::Other);
                    entry.note = "a model with no GGUF layer (safetensors), in Docker Model Runner's store".into();
                    entry
                }
            };
            entry.bytes = bytes;
            if parts.len() > 1 {
                entry.parts = parts.clone();
            }
            found.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::locations::Kind;
    use crate::scan::tests::{location, write};
    use crate::scan::{Format, scan};
    use crate::test_dir;

    #[test]
    fn one_model_per_tag_from_its_gguf_layers_and_not_while_downloading() {
        let root = test_dir("docker-models");
        write(&root.join("layout.json"), br#"{"version": "1.0.0"}"#);
        let (a, b, c, d) = ("a".repeat(64), "b".repeat(64), "c".repeat(64), "d".repeat(64));
        write(
            &root.join("models.json"),
            format!(
                r#"{{"models": [{{"id": "sha256:{a}", "tags": ["ai/smollm2:latest", "ai/smollm2:360M-Q4_K_M"]}},
                               {{"id": "sha256:{c}", "tags": ["ai/partial:latest"]}}]}}"#
            )
            .as_bytes(),
        );
        let layer = |t: &str, h: &str| format!(r#"{{"mediaType": "{t}", "digest": "sha256:{h}"}}"#);
        let gguf = "application/vnd.docker.ai.gguf.v3";
        write(
            &root.join("manifests/sha256").join(&a),
            format!(r#"{{"layers": [{}, {}]}}"#, layer(gguf, &b), layer("application/vnd.docker.ai.license", &d))
                .as_bytes(),
        );
        write(&root.join("blobs/sha256").join(&b), b"1234");
        write(&root.join("blobs/sha256").join(&d), b"MIT");
        write(&root.join("manifests/sha256").join(&c), format!(r#"{{"layers": [{}]}}"#, layer(gguf, &c)).as_bytes());
        write(&root.join("blobs/sha256").join(format!("{c}.incomplete")), b"1");

        let entries = scan(&[location(&root, Kind::DockerModels)]);
        let found: Vec<(&str, Format, u64)> = entries.iter().map(|e| (e.id.as_str(), e.format, e.bytes)).collect();
        assert_eq!(
            found,
            [("docker:ai/smollm2:latest", Format::Gguf, 4), ("docker:ai/smollm2:360M-Q4_K_M", Format::Gguf, 4)]
        );
    }
}
