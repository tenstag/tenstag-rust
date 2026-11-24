//! # Jan's models
//!
//! Jan keeps each model's settings in `model.yml`, in a folder named by the model's id, and saves a downloaded file
//! under a generic name; a file imported in place stays where it was:
//!
//! ```text
//!   <data folder>/llamacpp/models/<model id>/model.yml       model_path: llamacpp/models/<model id>/model.gguf
//!   <data folder>/llamacpp/models/<model id>/model.gguf      the model (no name or quant tag of its own)
//!   <data folder>/llamacpp/models/<model id>/mmproj.gguf     its projector; mtp.gguf a draft model: side files
//!   <data folder>/llamacpp/models/<other id>/model.gguf.tmp  a download in progress (no model.yml yet): not a model
//! ```
//!
//! Entries are named `jan:<model id>`. Spec: tenstag specs/stores/jan.md JAN-001 to JAN-007 (SUG-046). The data
//! folder is found in locations.rs.

use std::path::{Path, PathBuf};

use super::{Entry, Format, file_name, new_entry, relative};
use crate::files::GgufName;

/// The models of Jan's data folder `data`, `model.yml` files looked for `depth` parts below `llamacpp/models/`.
///
/// SPEC tenstag specs/stores/jan.md JAN-003 (IMPL) and JAN-007 (SHOULD, SUG-046): one model per `model.yml` at any
/// depth (an id may hold `/`), its GGUF file `model_path` (relative to the data folder, or absolute); a folder without
/// `model.yml` is not finished (JAN-004: `.tmp` and `.url` files are downloads in progress). The tag is taken from the
/// model id, as `model.gguf` has none. JAN-006 (MUST): nothing is written; settings, secrets and threads are not
/// opened.
pub(super) fn scan(data: &Path, depth: usize, found: &mut Vec<Entry>) {
    let models = data.join("llamacpp").join("models");
    let mut visited = std::collections::HashSet::new();
    walk(data, &models, &models, 0, depth, &mut visited, found);
}

fn walk(
    data: &Path,
    models: &Path,
    dir: &Path,
    level: usize,
    depth: usize,
    visited: &mut std::collections::HashSet<PathBuf>,
    found: &mut Vec<Entry>,
) {
    let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if level > depth || !visited.insert(real) {
        return;
    }
    if let Ok(text) = std::fs::read_to_string(dir.join("model.yml")) {
        found.extend(model(data, &relative(models, dir), &text));
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut folders: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
    folders.sort();
    for folder in folders.iter().filter(|f| !file_name(f).starts_with('.')) {
        walk(data, models, folder, level + 1, depth, visited, found);
    }
}

/// The entry of the model `id` whose `model.yml` is `text`, when its GGUF file is there.
fn model(data: &Path, id: &str, text: &str) -> Option<Entry> {
    let file = PathBuf::from(yaml_scalar(text, "model_path")?);
    let path = if file.is_absolute() { file } else { data.join(file) };
    let bytes = std::fs::metadata(&path).ok().filter(|m| m.is_file())?.len();
    let mut entry = new_entry(format!("jan:{id}"), path, Format::Gguf);
    entry.quant = GgufName::parse(&format!("{id}.gguf")).map(|n| n.tag).unwrap_or_default();
    entry.bytes = bytes;
    Some(entry)
}

/// The value of the top-level key `key` of a YAML mapping, when it is a plain or quoted scalar on the key's line.
///
/// DECISION: Jan writes `model.yml` as a flat mapping of scalars, so this reads only `key: value` lines at the start of
/// a line, with `'...'` (a doubled `''` is one `'`) or `"..."` (`\\` and `\"` escapes) quoting, rather than adding a
/// YAML library for two keys. A value written any other way (a block, a flow collection) gives `None`, and the model
/// is not listed.
fn yaml_scalar(text: &str, key: &str) -> Option<String> {
    let line = text.lines().find(|l| l.strip_prefix(key).is_some_and(|rest| rest.starts_with(':')))?;
    let value = line[key.len() + 1..].trim();
    if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        return Some(inner.replace("''", "'"));
    }
    if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match (c, c == '\\') {
                (_, true) => match chars.next()? {
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    _ => return None,
                },
                (c, false) => out.push(c),
            }
        }
        return Some(out);
    }
    let plain = value.split(" #").next().unwrap_or(value).trim();
    (!plain.is_empty() && !plain.starts_with(['|', '>', '[', '{', '&', '*', '!'])).then(|| plain.to_string())
}

#[cfg(test)]
mod tests {
    use super::yaml_scalar;
    use crate::locations::Kind;
    use crate::scan::scan;
    use crate::scan::tests::{location, write};
    use crate::test_dir;

    #[test]
    fn one_model_per_model_yml_and_unfinished_downloads_are_not_models() {
        let data = test_dir("jan");
        let m = data.join("llamacpp/models");
        write(&m.join("Qwen3-0.6B-Q8_0/model.gguf"), b"123");
        write(&m.join("Qwen3-0.6B-Q8_0/mmproj.gguf"), b"1");
        write(
            &m.join("Qwen3-0.6B-Q8_0/model.yml"),
            b"model_path: llamacpp/models/Qwen3-0.6B-Q8_0/model.gguf\nname: 'Qwen3 0.6B'\nsize_bytes: 3\n",
        );
        write(&m.join("unsloth/gemma-3-1b/model.gguf.tmp"), b"1");
        write(&data.join("elsewhere/mine.gguf"), b"12345");
        let mine = data.join("elsewhere/mine.gguf");
        write(&m.join("imported/mine/model.yml"), format!("model_path: \"{}\"\n", mine.display()).as_bytes());

        let entries = scan(&[location(&data, Kind::Jan)]);
        let found: Vec<(&str, &str, u64)> =
            entries.iter().map(|e| (e.id.as_str(), e.quant.as_str(), e.bytes)).collect();
        assert_eq!(found, [("jan:Qwen3-0.6B-Q8_0", "Q8_0", 3), ("jan:imported/mine", "", 5)]);
    }

    #[test]
    fn scalars_are_read_plain_or_quoted() {
        let text = "name: \"a \\\"b\\\"\"\nmodel_path: 'it''s.gguf'\nsize_bytes: 3 # bytes\nlist: [1]\n";
        assert_eq!(yaml_scalar(text, "name").as_deref(), Some("a \"b\""));
        assert_eq!(yaml_scalar(text, "model_path").as_deref(), Some("it's.gguf"));
        assert_eq!(yaml_scalar(text, "size_bytes").as_deref(), Some("3"));
        assert_eq!(yaml_scalar(text, "list"), None);
        assert_eq!(yaml_scalar("  name: x\n", "name"), None, "not at the top level");
    }
}
