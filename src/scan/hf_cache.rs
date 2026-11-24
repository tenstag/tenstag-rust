//! # The Hugging Face cache's repository folders
//!
//! A repository folder `models--<org>--<name>` holds `refs/<revision>` (a commit), `blobs/<blob>` and
//! `snapshots/<commit>/<path>`. Each snapshot's models are entries:
//!
//! ```text
//!   the snapshot refs/main points at      org/name:Q4_K_M  (a GGUF file by its tag)   org/name  (a directory)
//!   a snapshot another ref points at      org/name:Q4_K_M@<ref>                       org/name@<ref>
//!   a snapshot no ref points at           org/name:Q4_K_M@<commit>                    org/name@<commit>
//! ```
//!
//! Spec: specs/stores/hf-hub-cache.md HFC-004 (MUST, the layout), HFC-005 (MUST: a snapshot entry is read through
//! whatever it is, a relative or absolute link or a regular file, and no blob is required), HFC-006 (IMPL: partial
//! and side files are not files of the snapshot), HFC-010 (IMPL: the tag of a file name), HFC-015 (IMPL: which
//! snapshot a name without a revision means; `names::Library::cached` uses it).
//!
//! DECISION: every snapshot is listed, not only the one a name without a revision reaches (HFC-015), so that a
//! model downloaded at a branch, tag or commit other than main can be named; the `@` in its id says how.

use std::path::Path;

use super::{Entry, HubOrigin, directory_entry, gguf_entry, list_files, new_entry, single};
use crate::files::GgufName;

/// The entries of the repository folder `dir` of repository `repo`.
pub(super) fn repository(dir: &Path, repo: &str, found: &mut Vec<Entry>) {
    // The refs: main first, then the others by name (HFC-015 leaves their order to the directory; a name is
    // defined).
    let mut refs: Vec<(String, String)> = Vec::new();
    list_refs(&dir.join("refs"), "", &mut refs);
    refs.sort_by(|a, b| (a.0 != "main", &a.0).cmp(&(b.0 != "main", &b.0)));
    let mut commits: Vec<String> = refs.iter().map(|(_, c)| c.clone()).collect();
    let mut others: Vec<String> = std::fs::read_dir(dir.join("snapshots"))
        .map(|read| read.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect())
        .unwrap_or_default();
    others.sort();
    commits.extend(others);
    let mut seen = std::collections::HashSet::new();
    commits.retain(|c| seen.insert(c.clone()));

    for commit in commits {
        let snapshot = dir.join("snapshots").join(&commit);
        if !snapshot.is_dir() {
            continue;
        }
        let names: Vec<String> = refs.iter().filter(|(_, c)| *c == commit).map(|(n, _)| n.clone()).collect();
        let at = match names.first() {
            Some(name) if name == "main" => String::new(),
            Some(name) => format!("@{name}"),
            None => format!("@{commit}"),
        };
        let origin = |file: &str| HubOrigin {
            repo: repo.to_string(),
            commit: commit.clone(),
            refs: names.clone(),
            file: file.to_string(),
        };
        // SPEC tenstag specs/stores/hf-hub-cache.md HFC-017 (IMPL): a snapshot holding model_index.json is a diffusers
        // pipeline, opened as a whole; HFC-019 (IMPL): one without model_index.json or config.json is no model
        // directory, and may hold single files fetched alone. The first of model_index.json, config.json,
        // adapter_config.json gives a directory's kind (`directory_entry`).
        let whole = directory_entry(format!("{repo}{at}"), &snapshot);
        let pipeline = snapshot.join("model_index.json").is_file();
        let mut files = Vec::new();
        list_files(&snapshot, &snapshot, &mut files);
        files.sort();
        for file in files.iter().filter(|_| !pipeline) {
            let name = file.rsplit('/').next().unwrap_or(file);
            if whole.is_none() && !file.ends_with(".gguf") {
                // A single file fetched alone (HFC-019): listed whatever its size, as it was asked for by name.
                let Some(format) = single::format_of(name, false) else { continue };
                if !single::first_part(name) {
                    continue;
                }
                let path = snapshot.join(file);
                let mut entry = new_entry(format!("{repo}/{file}{at}"), path.clone(), format);
                (entry.bytes, entry.hub) = (single::bytes(&path), Some(origin(file)));
                found.push(entry);
                continue;
            }
            let tag = GgufName::parse(file).map(|n| n.tag).unwrap_or_default();
            let id = if tag.is_empty() { format!("{repo}/{file}{at}") } else { format!("{repo}:{tag}{at}") };
            let Some(mut entry) = gguf_entry(id, &snapshot.join(file)) else { continue };
            // Two files with the same tag (two models in one repository): the second is named by its file.
            if found.iter().any(|e| e.id == entry.id) {
                entry.id = format!("{repo}/{file}{at}");
            }
            entry.hub = Some(origin(file));
            found.push(entry);
        }
        if let Some(mut entry) = whole {
            entry.hub = Some(origin(""));
            found.push(entry);
        }
    }
}

/// The refs under `dir` (a branch may hold `/`, as `refs/pr/1`), each with the commit it holds.
fn list_refs(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.filter_map(|e| e.ok()) {
        let name = format!("{prefix}{}", e.file_name().to_string_lossy());
        if e.path().is_dir() {
            list_refs(&e.path(), &format!("{name}/"), out);
        } else if let Ok(commit) = std::fs::read_to_string(e.path()) {
            out.push((name, commit.trim().to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::locations::Kind;
    use crate::scan::tests::{hub_snapshot, location};
    use crate::scan::{Format, scan};
    use crate::test_dir;

    #[test]
    fn every_snapshot_is_listed_and_named_by_how_it_is_reached() {
        let root = test_dir("hf-cache");
        let (main, v1, loose) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
        hub_snapshot(&root, "Org/M-GGUF", &main, &["main"], &[("M-Q4_K_M.gguf", b"12"), ("M-Q8_0.gguf", b"123")]);
        hub_snapshot(&root, "Org/M-GGUF", &v1, &["v1", "abc1234"], &[("M-Q8_0.gguf", b"1234")]);
        hub_snapshot(&root, "Org/M-GGUF", &loose, &[], &[("M-Q8_0.gguf", b"12345")]);
        hub_snapshot(&root, "Org/D", &main, &["main"], &[("config.json", b"{}"), ("model.safetensors", b"1")]);
        std::fs::write(root.join("models--Org--M-GGUF/snapshots").join(&main).join("M-F16.gguf.incomplete"), b"x")
            .unwrap();

        let entries = scan(&[location(&root, Kind::Directory)]);
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        let at_loose = format!("Org/M-GGUF:Q8_0@{loose}");
        assert_eq!(
            ids,
            ["Org/D", "Org/M-GGUF:Q4_K_M", "Org/M-GGUF:Q8_0", "Org/M-GGUF:Q8_0@abc1234", at_loose.as_str()]
        );
        let v1_refs = &entries[3].hub.as_ref().unwrap().refs;
        assert_eq!(v1_refs, &["abc1234", "v1"]);
        assert!(entries[4].hub.as_ref().unwrap().refs.is_empty());
    }

    #[test]
    fn a_pipeline_snapshot_is_one_model_and_single_files_are_their_own() {
        let root = test_dir("hf-cache-image");
        let main = "d".repeat(40);
        hub_snapshot(
            &root,
            "Org/Pipe",
            &main,
            &["main"],
            &[
                ("model_index.json", b"{}"),
                ("unet/config.json", b"{}"),
                ("unet/diffusion_pytorch_model.fp16.safetensors", b"1234"),
                ("vae/diffusion_pytorch_model.fp16.safetensors", b"56"),
            ],
        );
        hub_snapshot(
            &root,
            "Org/Single",
            &main,
            &["main"],
            &[("sd_xl_base_1.0.safetensors", b"123"), ("README.md", b"x")],
        );

        let entries = scan(&[location(&root, Kind::Directory)]);
        let found: Vec<(&str, Format, u64)> = entries.iter().map(|e| (e.id.as_str(), e.format, e.bytes)).collect();
        assert_eq!(
            found,
            [("Org/Pipe", Format::Diffusers, 6), ("Org/Single/sd_xl_base_1.0.safetensors", Format::Safetensors, 3)],
            "the pipeline's component files are not models of their own"
        );
        assert_eq!(entries[1].hub.as_ref().unwrap().file, "sd_xl_base_1.0.safetensors");
    }
}
