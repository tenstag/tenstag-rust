//! # Changing the configuration file for the person
//!
//! Adding a location, removing one, turning the defaults on or off. The file is the person's, shared by every program
//! using tenstag, and may hold their comments and their own order. So a change touches only the lines it must, keeps
//! every other line exactly as written, and is checked before it is written: the new text, read as TOML, must say
//! exactly what the old one said with the one change made. A file that cannot be read, or says something in a form
//! these line rules do not follow, is refused, never rewritten.
//!
//! DECISION: line rules, not a TOML editing library (`toml_edit`), so that every implementation writes the same text
//! (the specification's vectors compare it) and so that tenstag-python stays without dependencies.
//!
//! Spec: tenstag specs/configuration.md CFG-010 (MUST: the file, the lock, the check, the atomic write), CFG-011
//! (MUST: adding), CFG-012 (MUST: removing), CFG-013 (MUST: defaults).

use std::fs::File;
use std::path::{Path, PathBuf};

use crate::env::Environment;
use crate::error::{Error, Result};
use crate::locations::{DEFAULT_MIN_SIZE, Kind, Location, config_path, config_text_locations, utc_text};

/// What adding a location to the configuration file did ([`Locations::add_to_config`](crate::Locations::add_to_config)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddedToConfig {
    /// It was added: the location as it is now read, with `added_by` and `added`.
    Added(Location),
    /// The file lists the folder already, and nothing was written: the location there, with its `added_by` and
    /// `added`.
    AlreadyListed(Location),
}

/// The time now in UTC, as CFG-011 writes it.
fn now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    utc_text(seconds)
}

/// CFG-011: a TOML basic string: `\` and `"` escaped, U+0000 to U+001F and U+007F as `\uXXXX`.
fn basic_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '\\' | '"' => {
                out.push('\\');
                out.push(ch);
            }
            '\u{0}'..='\u{1f}' | '\u{7f}' => out.push_str(&format!("\\u{:04X}", u32::from(ch))),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// CFG-011: a folder compared as CFG-006 compares, after CFG-008's `~`.
fn key(env: &Environment, path: &str) -> PathBuf {
    let path = env.expand(path);
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn trimmed(line: &str) -> &str {
    line.trim_start_matches([' ', '\t'])
}

fn is_empty(line: &str) -> bool {
    trimmed(line).is_empty()
}

fn is_comment(line: &str) -> bool {
    trimmed(line).starts_with('#')
}

fn is_table(line: &str) -> bool {
    trimmed(line).starts_with('[')
}

fn is_location(line: &str) -> bool {
    trimmed(line).starts_with("[[location]]")
}

fn config_error(path: &Path, message: impl Into<String>) -> Error {
    Error::Config { path: path.to_path_buf(), message: message.into() }
}

/// The text as TOML; the file was checked to be TOML already.
fn table(path: &Path, text: &str) -> Result<toml::Table> {
    text.parse::<toml::Table>().map_err(|e| config_error(path, e.to_string()))
}

/// CFG-010 step 5: the new text, as TOML, is exactly `expected`.
fn check(path: &Path, new: &str, expected: &toml::Table) -> Result<()> {
    match new.parse::<toml::Table>() {
        Ok(got) if &got == expected => Ok(()),
        _ => Err(refused(path)),
    }
}

fn refused(path: &Path) -> Error {
    config_error(
        path,
        "it says this in a form tenstag cannot change without rewriting it; change it by hand, with one \
         `[[location]]` table for each location",
    )
}

/// The `location` array of a TOML table, or an empty one.
fn location_array(table: &toml::Table) -> Vec<toml::Value> {
    table.get("location").and_then(toml::Value::as_array).cloned().unwrap_or_default()
}

/// Every location is a `[[location]]` header line, so that the n-th header is the n-th location.
fn count_check(path: &Path, lines: &[&str], table: &toml::Table) -> Result<()> {
    if lines.iter().filter(|l| is_location(l)).count() == location_array(table).len() {
        Ok(())
    } else {
        Err(refused(path))
    }
}

/// One change: what the edit decided.
struct Edit<T> {
    /// The new text, or `None` when there is nothing to change.
    text: Option<String>,
    result: T,
}

/// CFG-010: runs `edit(old text, old TOML, old locations)` on the configuration file under its lock, and writes the
/// new text when there is one. The edit checks that nothing else changed.
fn change<T>(env: &Environment, edit: impl Fn(&str, &toml::Table, &[Location]) -> Result<Edit<T>>) -> Result<T> {
    let path = config_path(env);
    if env.path("TENSTAG_CONFIG").is_some() && !path.is_file() {
        return Err(config_error(&path, "TENSTAG_CONFIG names it, and it is not there"));
    }
    let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
    if !folder.as_os_str().is_empty() && !folder.is_dir() {
        // Nothing exists yet: create the folders only when there is something to write.
        let first = edit("", &toml::Table::new(), &[])?;
        if first.text.is_none() {
            return Ok(first.result);
        }
        std::fs::create_dir_all(&folder).map_err(|e| Error::io(&folder, e))?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let lock_path = path.with_file_name(format!("{name}.lock"));
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| Error::io(&lock_path, e))?;
    lock.lock().map_err(|e| Error::io(&lock_path, e))?;
    let text = match std::fs::read(&path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| config_error(&path, "not UTF-8: it is not changed"))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Error::io(&path, e)),
    };
    // Step 3: a file that breaks CFG-002 to CFG-004 or CFG-009 is an error, and is never changed.
    let read = config_text_locations(&path, env, &text)?;
    let done = edit(&text, &table(&path, &text)?, &read)?;
    if let Some(new) = &done.text {
        crate::fingerprint::write_replacing(&path, new.as_bytes()).map_err(|e| Error::io(&path, e))?;
    }
    drop(lock);
    Ok(done.result)
}

/// CFG-011: `location` added at the end of the file, unless the file lists its folder already.
pub(crate) fn add(
    env: &Environment,
    location: &Location,
    added_by: Option<&str>,
    now_text: Option<&str>,
) -> Result<AddedToConfig> {
    let path = config_path(env);
    let given = location
        .path
        .to_str()
        .ok_or_else(|| config_error(&path, format!("{}: a path that is not UTF-8", location.path.display())))?
        .to_string();
    if !(location.path.is_absolute() || given == "~" || given.starts_with("~/")) {
        return Err(config_error(
            &path,
            format!(
                "{given}: a location added to the configuration has an absolute path, or one starting with ~/ (a \
                 relative one would mean a different folder to each program)"
            ),
        ));
    }
    if let Some(owner) = added_by
        && !crate::tags::is_owner(owner)
    {
        return Err(config_error(&path, format!("added_by {owner:?} is not an owner name (tags.md TAG-009)")));
    }
    let kind = location.kind;
    let mut lines = vec!["[[location]]".to_string(), format!("path = {}", basic_string(&given))];
    if kind != Kind::Directory {
        lines.push(format!("kind = {}", basic_string(kind_name(kind))));
    }
    if kind.walked()
        && let Some(depth) = location.depth.filter(|d| Some(*d) != kind.default_depth())
    {
        lines.push(format!("depth = {depth}"));
    }
    if kind == Kind::Directory && location.min_size != DEFAULT_MIN_SIZE {
        lines.push(format!("min_size = {}", location.min_size));
    }
    if let Some(owner) = added_by {
        lines.push(format!("added_by = {}", basic_string(owner)));
    }
    lines.push(format!("added = {}", now_text.map_or_else(now, str::to_string)));
    let block = lines.join("\n") + "\n";

    change(env, |text, old, read| {
        let wanted = key(env, &given);
        if let Some(existing) = read.iter().find(|l| key(env, &l.path.to_string_lossy()) == wanted) {
            return Ok(Edit { text: None, result: AddedToConfig::AlreadyListed(existing.clone()) });
        }
        let mut new = text.to_string();
        if !new.is_empty() {
            if !new.ends_with('\n') {
                new.push('\n');
            }
            new.push('\n');
        }
        new.push_str(&block);
        let mut expected = old.clone();
        let mut array = location_array(old);
        array.extend(location_array(&table(&path, &block)?));
        expected.insert("location".into(), toml::Value::Array(array));
        count_check(&path, &text.split('\n').collect::<Vec<_>>(), old)?;
        check(&path, &new, &expected)?;
        let added = config_text_locations(&path, env, &block)?.remove(0);
        Ok(Edit { text: Some(new), result: AddedToConfig::Added(added) })
    })
}

/// CFG-012: every `[[location]]` of `folder` removed, with the comments directly above it.
pub(crate) fn remove(env: &Environment, folder: &Path) -> Result<Option<Location>> {
    let path = config_path(env);
    let wanted = key(env, &folder.to_string_lossy());
    change(env, |text, old, read| {
        let gone: Vec<usize> =
            (0..read.len()).filter(|&i| key(env, &read[i].path.to_string_lossy()) == wanted).collect();
        let Some(&first) = gone.first() else {
            return Ok(Edit { text: None, result: None });
        };
        let mut lines: Vec<&str> = text.split('\n').collect();
        if lines.last() == Some(&"") {
            lines.pop();
        }
        count_check(&path, &lines, old)?;
        let headers: Vec<usize> = (0..lines.len()).filter(|&i| is_location(lines[i])).collect();
        for &index in gone.iter().rev() {
            let header = headers[index];
            let mut start = header;
            while start > 0 && is_comment(lines[start - 1]) {
                start -= 1;
            }
            let mut end = header + 1;
            while end < lines.len() && !is_table(lines[end]) {
                end += 1;
            }
            while end > header + 1 && (is_empty(lines[end - 1]) || is_comment(lines[end - 1])) {
                end -= 1;
            }
            lines.drain(start..end);
            if start > 0 && start < lines.len() && is_empty(lines[start - 1]) && is_empty(lines[start]) {
                lines.remove(start);
            }
        }
        while lines.last().is_some_and(|l| is_empty(l)) {
            lines.pop();
        }
        let new = if lines.is_empty() { String::new() } else { lines.join("\n") + "\n" };
        let mut expected = old.clone();
        let kept: Vec<toml::Value> =
            location_array(old).into_iter().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, v)| v).collect();
        if kept.is_empty() {
            expected.remove("location");
        } else {
            expected.insert("location".into(), toml::Value::Array(kept));
        }
        check(&path, &new, &expected)?;
        Ok(Edit { text: Some(new), result: Some(read[first].clone()) })
    })
}

/// CFG-013: the `defaults` line's value replaced, or `defaults = false` put first. Whether the file changed.
pub(crate) fn set_defaults(env: &Environment, on: bool) -> Result<bool> {
    let path = config_path(env);
    let value = if on { "true" } else { "false" };
    change(env, |text, old, _| {
        let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
        let mut found = false;
        for line in &mut lines {
            if is_table(line) {
                break;
            }
            let Some(rest) = trimmed(line).strip_prefix("defaults") else { continue };
            let after_key = rest.trim_start_matches([' ', '\t']);
            let Some(after_equals) = after_key.strip_prefix('=') else { continue };
            let current = after_equals.trim_start_matches([' ', '\t']);
            let Some(old_value) = ["true", "false"].into_iter().find(|v| current.starts_with(v)) else { continue };
            if old_value == value {
                return Ok(Edit { text: None, result: false });
            }
            let at = line.len() - current.len();
            *line = format!("{}{value}{}", &line[..at], &current[old_value.len()..]);
            found = true;
            break;
        }
        let new = if found {
            lines.join("\n")
        } else if on {
            return Ok(Edit { text: None, result: false });
        } else {
            format!("defaults = false\n{text}")
        };
        let mut expected = old.clone();
        expected.insert("defaults".into(), toml::Value::Boolean(on));
        check(&path, &new, &expected)?;
        Ok(Edit { text: Some(new), result: true })
    })
}

/// A kind's name in the configuration file (CFG-003).
pub(crate) fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Directory => "directory",
        Kind::LmStudio => "lmstudio",
        Kind::Ollama => "ollama",
        Kind::Gpt4All => "gpt4all",
        Kind::ModelScope => "modelscope",
        Kind::Jan => "jan",
        Kind::DockerModels => "docker",
        Kind::RamaLama => "ramalama",
    }
}

#[cfg(test)]
mod tests {
    use crate::{AddedToConfig, Environment, Location, Locations, Source};
    use std::collections::HashMap;

    #[test]
    fn a_program_adds_and_removes_a_folder_and_turns_the_defaults_off() {
        let root = std::fs::canonicalize(crate::test_dir("config-write-api")).unwrap();
        let s = |p: &str| root.join(p).to_string_lossy().to_string();
        let env = Environment::from_map(HashMap::from([
            ("HOME".to_string(), s("home")),
            ("XDG_CONFIG_HOME".to_string(), s("conf")),
            ("TENSTAG_CACHE_DIR".to_string(), s("cache")),
        ]));
        let locations = Locations::load_from(env.clone()).unwrap();
        assert_eq!(locations.config_file, None);
        assert_eq!(locations.config_path(), root.join("conf/tenstag/config.toml"));
        let defaults = locations.default_locations();
        assert_eq!(defaults, locations.read);

        let models = root.join("models");
        let AddedToConfig::Added(added) =
            locations.add_to_config(&Location::folder(&models).depth(3), Some("goldnode/tensrun")).unwrap()
        else {
            panic!("not added")
        };
        assert_eq!(added.added.as_ref().map(String::len), Some(20));
        let again = locations.add_to_config(&Location::folder(format!("{}/", models.display())), None).unwrap();
        assert!(matches!(again, AddedToConfig::AlreadyListed(l) if l.added_by.as_deref() == Some("goldnode/tensrun")));
        assert!(locations.set_defaults(false).unwrap());
        assert!(!locations.set_defaults(false).unwrap());

        let reloaded = Locations::load_from(env).unwrap();
        assert_eq!(reloaded.read.len(), 1);
        let first = &reloaded.read[0];
        assert_eq!(
            (&first.path, first.depth, first.added_by.as_deref(), first.source),
            (&models, Some(3), Some("goldnode/tensrun"), Source::Config)
        );
        assert_eq!(reloaded.default_locations().len(), defaults.len(), "listed even when off");

        let removed = reloaded.remove_from_config(&models).unwrap().unwrap();
        assert_eq!(removed.added_by.as_deref(), Some("goldnode/tensrun"));
        assert_eq!(reloaded.remove_from_config(&models).unwrap(), None);
        assert_eq!(std::fs::read_to_string(root.join("conf/tenstag/config.toml")).unwrap(), "defaults = false\n");
        assert!(reloaded.add_to_config(&Location::folder("relative"), None).is_err());
    }

    #[test]
    fn utc_text_round_trips_dates() {
        assert_eq!(crate::locations::utc_text(0), "1970-01-01T00:00:00Z");
        assert_eq!(crate::locations::utc_text(1_795_545_498), "2026-11-24T18:38:18Z");
        assert_eq!(crate::locations::utc_text(951_782_400), "2000-02-29T00:00:00Z");
    }
}
