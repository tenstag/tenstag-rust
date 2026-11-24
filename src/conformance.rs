//! # The tenstag specification's hand-written cases, run against this implementation
//!
//! tests/vectors/listing.json, configuration.json, config-writing.json and names.json are copies of the specification's
//! specs/test-vectors/ at the commit spec-pin.toml names (tenstag tools/cases.py writes them, each expected result
//! written by hand from the specification). Each case's files are built in an empty folder and the results compared.
//!
//! Spec: tenstag specs/configuration.md, locations.md, listing.md and names.md.
//!
//! Test code only: nothing here is part of the library.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use crate::config_write::kind_name;
use crate::locations::Location;
use crate::{Environment, Error, Format, HubRef, Kind, Library, Locations};

/// `{root}` in `text` made the case's folder.
fn at(text: &str, root: &Path) -> String {
    text.replace("{root}", &root.to_string_lossy())
}

/// Builds a case's `files` under `root`. Returns false when a file needs what this system lacks (a link off Unix).
fn build(files: &Value, root: &Path) -> bool {
    for file in files.as_array().unwrap() {
        let path = root.join(file["path"].as_str().unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if let Some(n) = file["size"].as_u64() {
            std::fs::write(&path, vec![0u8; n as usize]).unwrap();
        } else if let Some(text) = file["text"].as_str() {
            std::fs::write(&path, at(text, root)).unwrap();
        } else if let Some(json) = file.get("json") {
            std::fs::write(&path, at(&json.to_string(), root)).unwrap();
        } else if let Some(target) = file["link"].as_str() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &path).unwrap();
            #[cfg(not(unix))]
            {
                let _ = target;
                return false;
            }
        }
    }
    true
}

fn kind(name: &str) -> Kind {
    match name {
        "directory" => Kind::Directory,
        "lmstudio" => Kind::LmStudio,
        "ollama" => Kind::Ollama,
        "gpt4all" => Kind::Gpt4All,
        "modelscope" => Kind::ModelScope,
        "jan" => Kind::Jan,
        "docker" => Kind::DockerModels,
        "ramalama" => Kind::RamaLama,
        other => panic!("kind {other}"),
    }
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::Gguf => "gguf",
        Format::HfDirectory => "hf-directory",
        Format::Safetensors => "safetensors",
        Format::Checkpoint => "checkpoint",
        Format::Diffusers => "diffusers",
        Format::Adapter => "adapter",
        Format::Other => "other",
    }
}

/// A path below `root` as the vectors write it: relative, with `/`.
fn rel(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

#[test]
fn listing_and_names_cases() {
    let vectors: Value = serde_json::from_str(include_str!("../tests/vectors/listing.json")).unwrap();
    for (i, case) in vectors["cases"].as_array().unwrap().iter().enumerate() {
        let why = case["why"].as_str().unwrap();
        let root = crate::test_dir(&format!("conformance-listing-{i}"));
        if !build(&case["files"], &root) {
            continue;
        }
        let mut locations = Locations::bare_from(Environment::from_map(HashMap::from([(
            "TENSTAG_CACHE_DIR".to_string(),
            root.join(".cache").to_string_lossy().to_string(),
        )])));
        for l in case["locations"].as_array().unwrap() {
            let mut location = Location::new(root.join(l["path"].as_str().unwrap()), kind(l["kind"].as_str().unwrap()));
            if let Some(depth) = l["depth"].as_u64() {
                location = location.depth(depth as usize);
            }
            if let Some(min) = l["min_size"].as_u64() {
                location = location.min_size(min);
            }
            locations.add(location);
        }
        if let Some(names) = case["names"].as_object() {
            locations.names = names.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        }
        let mut library = Library::new(locations);
        let entries = library.entries().to_vec();
        let got: Vec<Value> = entries
            .iter()
            .map(|e| {
                let mut v = serde_json::json!({
                    "id": e.id, "format": format_name(e.format), "quant": e.quant, "bytes": e.bytes,
                    "path": rel(&e.path, &root),
                    "files": e.files().iter().map(|f| rel(f, &root)).collect::<Vec<_>>(),
                });
                if let Some(h) = &e.hub {
                    v["hub"] = serde_json::json!({"repo": h.repo, "commit": h.commit, "refs": h.refs, "file": h.file});
                }
                if !e.parts.is_empty() {
                    v["parts"] = e.parts.iter().map(|p| rel(p, &root)).collect::<Vec<_>>().into();
                }
                if !e.note.is_empty() {
                    v["note"] = e.note.clone().into();
                }
                v
            })
            .collect();
        let want: Vec<Value> = case["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| serde_json::from_str(&at(&e.to_string(), &root)).unwrap())
            .collect();
        for (g, w) in got.iter().zip(&want) {
            assert_eq!(g, w, "{why}");
        }
        let ids = |v: &[Value]| v.iter().map(|e| e["id"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();
        assert_eq!(ids(&got), ids(&want), "{why}");
        for r in case["resolve"].as_array().unwrap() {
            let name = at(r["name"].as_str().unwrap(), &root);
            match (library.resolve(&name), r["error"].as_str()) {
                (Ok(entry), None) => {
                    assert_eq!(entry.id, at(r["id"].as_str().unwrap(), &root), "{why}: {name}");
                    if let Some(format) = r["format"].as_str() {
                        assert_eq!(format_name(entry.format), format, "{why}: {name}");
                    }
                }
                (Err(Error::NotFound { .. }), Some("not found"))
                | (Err(Error::Ambiguous { .. }), Some("ambiguous")) => {}
                (other, want) => panic!("{why}: {name}: {other:?}, wanted {want:?} {}", r["id"]),
            }
        }
    }
}

#[test]
fn configuration_cases() {
    let vectors: Value = serde_json::from_str(include_str!("../tests/vectors/configuration.json")).unwrap();
    for (i, case) in vectors["cases"].as_array().unwrap().iter().enumerate() {
        let why = case["why"].as_str().unwrap();
        let platform = case["platform"].as_str().unwrap();
        let runs = platform == "any" || (platform == "unix" && cfg!(unix)) || cfg!(target_os = "linux");
        if !runs {
            continue;
        }
        let root = crate::test_dir(&format!("conformance-config-{i}"));
        // The folder as the code will see it, links resolved, so that paths compare.
        let root = std::fs::canonicalize(&root).unwrap();
        if !build(&case["files"], &root) {
            continue;
        }
        let env: HashMap<String, String> =
            case["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), at(v.as_str().unwrap(), &root))).collect();
        let result = Locations::load_from(Environment::from_map(env));
        if case["error"].as_bool() == Some(true) {
            assert!(matches!(result, Err(Error::Config { .. })), "{why}: {result:?}");
            continue;
        }
        let locations = result.unwrap_or_else(|e| panic!("{why}: {e}"));
        let got: Vec<Value> = locations
            .read
            .iter()
            .zip(case["read"].as_array().unwrap().iter().chain(std::iter::repeat(&Value::Null)))
            .map(|(l, want)| {
                let mut v =
                    serde_json::json!({"path": l.path.to_string_lossy(), "kind": kind_name(l.kind), "depth": l.depth});
                if want.get("min_size").is_some() {
                    v["min_size"] = l.min_size.into();
                }
                if want.get("added_by").is_some() {
                    v["added_by"] = l.added_by.clone().into();
                }
                if want.get("added").is_some() {
                    v["added"] = l.added.clone().into();
                }
                v
            })
            .collect();
        let want: Vec<Value> = case["read"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| serde_json::from_str(&at(&e.to_string(), &root)).unwrap())
            .collect();
        assert_eq!(got, want, "{why}");
        if let Some(names) = case["names"].as_object() {
            let want: Vec<(String, String)> =
                names.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().into())).collect();
            let got: Vec<(String, String)> = locations.names.clone().into_iter().collect();
            assert_eq!(got, want, "{why}");
        }
        if let Some(want) = case["cache"].as_str() {
            assert_eq!(locations.cache.to_string_lossy(), at(want, &root), "{why}: cache");
        }
    }
}

#[test]
fn repository_names() {
    let vectors: Value = serde_json::from_str(include_str!("../tests/vectors/names.json")).unwrap();
    for case in vectors["names"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let got = HubRef::parse(name);
        match case["repo"].as_str() {
            None => assert_eq!(got, None, "{name}"),
            Some(repo) => {
                let got = got.unwrap_or_else(|| panic!("{name}"));
                assert_eq!(
                    (got.repo.as_str(), got.quant.as_deref(), got.revision.as_deref()),
                    (repo, case["quant"].as_str(), case["revision"].as_str()),
                    "{name}"
                );
            }
        }
    }
}

/// A location as config-writing.json's rows give it.
fn config_row(l: &Location) -> Value {
    let mut v = serde_json::json!({"path": l.path.to_string_lossy(), "kind": kind_name(l.kind), "depth": l.depth,
                                   "added_by": l.added_by, "added": l.added});
    if l.min_size != crate::DEFAULT_MIN_SIZE {
        v["min_size"] = l.min_size.into();
    }
    v
}

#[test]
fn config_writing_cases() {
    let vectors: Value = serde_json::from_str(include_str!("../tests/vectors/config-writing.json")).unwrap();
    let now = vectors["now"].as_str().unwrap();
    for (i, case) in vectors["cases"].as_array().unwrap().iter().enumerate() {
        let why = case["why"].as_str().unwrap();
        if case["platform"] == "unix" && !cfg!(unix) {
            continue;
        }
        let root = std::fs::canonicalize(crate::test_dir(&format!("conformance-config-writing-{i}"))).unwrap();
        let toml_root = root.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        let text = |v: &Value| v.as_str().map(|t| t.replace("{root}", &toml_root));
        let file = root.join(case["file"].as_str().unwrap());
        if let Some(before) = text(&case["before"]) {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, before).unwrap();
        }
        let env: HashMap<String, String> =
            case["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), at(v.as_str().unwrap(), &root))).collect();
        let env = Environment::from_map(env);
        let change: Value = serde_json::from_str(&at(&case["change"].to_string(), &root)).unwrap();
        let result = if let Some(a) = change.get("add") {
            let kind = match a["kind"].as_str().unwrap_or("directory") {
                "directory" => Kind::Directory,
                "ollama" => Kind::Ollama,
                "gpt4all" => Kind::Gpt4All,
                other => panic!("{why}: kind {other}"),
            };
            let mut location = Location::new(a["path"].as_str().unwrap(), kind);
            if let Some(depth) = a["depth"].as_u64() {
                location = location.depth(depth as usize);
            }
            if let Some(bytes) = a["min_size"].as_u64() {
                location = location.min_size(bytes);
            }
            crate::config_write::add(&env, &location, a["added_by"].as_str(), Some(now)).map(|r| match r {
                crate::AddedToConfig::Added(l) => serde_json::json!({"added": config_row(&l)}),
                crate::AddedToConfig::AlreadyListed(l) => serde_json::json!({"listed": config_row(&l)}),
            })
        } else if let Some(path) = change["remove"].as_str() {
            crate::config_write::remove(&env, Path::new(path))
                .map(|r| serde_json::json!({"removed": r.as_ref().map(config_row)}))
        } else {
            crate::config_write::set_defaults(&env, change["defaults"].as_bool().unwrap())
                .map(|_| serde_json::json!({}))
        };
        let after = if case["error"].as_bool() == Some(true) {
            assert!(matches!(result, Err(Error::Config { .. })), "{why}: {result:?}");
            text(&case["before"])
        } else {
            let want: Value = serde_json::from_str(&at(&case["result"].to_string(), &root)).unwrap();
            assert_eq!(result.unwrap_or_else(|e| panic!("{why}: {e}")), want, "{why}");
            text(&case["after"])
        };
        assert_eq!(std::fs::read_to_string(&file).ok(), after, "{why}");
    }
}
