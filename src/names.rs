//! # Names: from what a person types to a model
//!
//! [`Library::resolve`] takes a name and tries, in order:
//!
//! ```text
//!   1. a path that exists               /data/m.gguf, ./Qwen3-0.6B
//!   2. a nickname from the configuration  qwen              -> its target, resolved again
//!   3. the id of a model that is not      ollama:qwen3:0.6b, gguf/qwen3-0.6b-q8_0
//!      from the Hugging Face cache, exactly (any case)
//!   4. a Hugging Face repository          org/name[:quant][@revision]: the model the cache holds, chosen
//!                                         as huggingface_hub and llama.cpp choose (`Library::cached`)
//!   5. any other id, exactly              Qwen/Qwen3-0.6B-GGUF/model.gguf
//!   6. a search                           every word of the name in the entry's id, or in its tags:
//!                                         "qwen3 0.6 q8", "qwen3 32768"
//! ```
//!
//! A search that matches more than one model is an error listing them. Nothing is downloaded: tenstag works on what
//! is on disk.
//!
//! The library also keeps tags on models (`tags`), by their identity (`fingerprint`).
//!
//! Spec: tenstag specs/names.md NAM-001 to NAM-006.
//!
//! The name grammar is tenstag's own (no store document defines one); a Hugging Face
//! repository is chosen from as specs/stores/hf-hub-cache.md HFC-010, HFC-013 and HFC-015 say.

use std::path::Path;

use crate::error::{Error, Result};
use crate::files::best_model;
use crate::fingerprint::{Cache, Identity, members, path_text, set_identity, sha256_file};
use crate::locations::Locations;
use crate::scan::{Entry, entry_for_path, scan};
use crate::tags::{Store, Tags, apply, is_owner, parse_tag};

/// A Hugging Face repository, and optionally the quant tag of one of its GGUF files and a revision:
/// `org/name[:quant][@revision]`, as llama.cpp's `-hf` takes it (with the revision added, which the Hub resolves:
/// specs/stores/hf-hub-cache.md HFC-013). DECISION: `hf.co/` or `huggingface.co/` in front is accepted, as Ollama
/// writes a Hub repository (specs/stores/ollama.md OLM-002); neither huggingface_hub nor llama.cpp accepts it
/// (HFC-014), and the spec leaves tenstag's names to tenstag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubRef {
    /// `org/name`.
    pub repo: String,
    /// The quant tag (`Q4_K_M`), if given.
    pub quant: Option<String>,
    /// A branch, tag or commit, if given (`main` otherwise).
    pub revision: Option<String>,
}

impl HubRef {
    /// Reads `org/name[:quant][@revision]`, or `None` when `name` is not one.
    pub fn parse(name: &str) -> Option<HubRef> {
        let name = name.strip_prefix("hf.co/").or_else(|| name.strip_prefix("huggingface.co/")).unwrap_or(name);
        let (rest, revision) = match name.rsplit_once('@') {
            Some((rest, rev)) if !rev.is_empty() && !rev.contains("..") && !rev.contains(char::is_whitespace) => {
                (rest, Some(rev.to_string()))
            }
            Some(_) => return None,
            None => (name, None),
        };
        let (repo, tag) = match rest.split_once(':') {
            Some((repo, tag)) => (repo, Some(tag)),
            None => (rest, None),
        };
        if !valid_repo(repo) {
            return None;
        }
        let tag_ok = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b));
        if tag.is_some_and(|t| !tag_ok(t)) {
            return None;
        }
        Some(HubRef { repo: repo.to_string(), quant: tag.map(str::to_string), revision })
    }
}

impl std::fmt::Display for HubRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.repo)?;
        if let Some(tag) = &self.quant {
            write!(f, ":{tag}")?;
        }
        if let Some(revision) = &self.revision {
            write!(f, "@{revision}")?;
        }
        Ok(())
    }
}

/// Whether `repo` is a repository id: one `/`; letters, digits and `_`, with `/`, `.` and `-` only between them.
///
/// SPEC tenstag specs/stores/hf-hub-cache.md HFC-014 (IMPL): llama.cpp's check (`is_valid_repo_id`: 1 to 256
/// characters, base characters `[A-Za-z0-9_]`, `/`, `.` and `-` only between them, exactly one `/`), which tenstag
/// follows, being the stricter of the two the rule gives, so that a name it accepts llama.cpp's `-hf` accepts too.
fn valid_repo(repo: &str) -> bool {
    let bytes = repo.as_bytes();
    let base = |i: usize| bytes.get(i).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
    repo.len() <= 256
        && repo.matches('/').count() == 1
        && bytes.iter().enumerate().all(|(i, b)| base(i) || (b"/.-".contains(b) && i > 0 && base(i - 1) && base(i + 1)))
}

/// The models in every location, the names that lead to them, and their tags.
pub struct Library {
    locations: Locations,
    entries: Option<Vec<Entry>>,
    store: Store,
    cache: Cache,
    only: Option<Rule>,
    /// The tag owner and its version (tenstag specs/tags.md TAG-009, TAG-010); none to read tags only.
    owner: Option<(String, u32)>,
}

/// A program's rule for which models it works with ([`Library::only`]).
type Rule = Box<dyn Fn(&Entry) -> bool + Send + Sync>;

impl Library {
    /// A library over `locations`. Nothing is scanned until a name needs it.
    pub fn new(locations: Locations) -> Library {
        let (store, cache) = (Store::new(&locations.cache), Cache::new(&locations.cache));
        Library { locations, entries: None, store, cache, only: None, owner: None }
    }

    /// A library over the locations of the process's environment and configuration file.
    ///
    /// # Errors
    ///
    /// As for [`Locations::load`].
    pub fn open() -> Result<Library> {
        Ok(Library::new(Locations::load()?))
    }

    /// As [`Library::open`], for the program that owns its tags as `owner` at `version` ([`Library::set_owner`]).
    ///
    /// # Errors
    ///
    /// As for [`Locations::load`] and [`Library::set_owner`].
    pub fn open_as(owner: &str, version: u32) -> Result<Library> {
        let mut library = Library::open()?;
        library.set_owner(owner, version)?;
        Ok(library)
    }

    /// From now on, the tags this library writes and reads as its own are `owner`'s, written at `version`.
    ///
    /// SPEC tenstag specs/tags.md TAG-009 (MUST): through this library a program writes only its own tags and reads
    /// anyone's (TAG-012: the name is not checked, and the files are plain files; it is no protection); an owner name
    /// is 1 to 128 bytes of lowercase ASCII letters, digits, `-`, `_`, `.` and `/`, starting with a letter or a digit.
    /// TAG-011 (SHOULD): one no other program will choose, a reverse domain name its maker controls
    /// (`com.example.chatrun`) or its GitHub repository in lower case (`example/chatrun`). TAG-010 (MUST): its own tags
    /// written at another version are seen as none, and its first write at this version replaces them; raise `version`
    /// when the way the program works out its tags changes, and every model is worked out again once. A library with no
    /// owner reads tags and writes none.
    ///
    /// # Errors
    ///
    /// [`Error::Tag`] when `owner` is not an owner name.
    pub fn set_owner(&mut self, owner: &str, version: u32) -> Result<()> {
        if !is_owner(owner) {
            return Err(Error::Tag(format!(
                "{owner:?} is not an owner name: 1 to 128 lowercase letters, digits, `-`, `_`, `.` and `/`, starting \
                 with a letter or a digit (a reverse domain name, com.example.myapp, or a GitHub user/repository)"
            )));
        }
        self.owner = Some((owner.to_string(), version));
        Ok(())
    }

    /// The tag owner and version, if one was given.
    pub fn owner(&self) -> Option<(&str, u32)> {
        self.owner.as_ref().map(|(o, v)| (o.as_str(), *v))
    }

    /// The owner, or an error saying a writing call needs one.
    fn me(&self, call: &str) -> Result<(String, u32)> {
        self.owner.clone().ok_or_else(|| {
            Error::Tag(format!("{call} needs a tag owner: open the library with Library::open_as, or set_owner"))
        })
    }

    /// The locations it reads.
    pub fn locations(&self) -> &Locations {
        &self.locations
    }

    /// From now on, lists, searches, tags queries and resolves names among the models `rule` keeps; every model by
    /// default. tenstag does not say what kind of model a file is; the program decides, from what an [`Entry`]
    /// gives (its format, its path, its id, the location it was found in). A language-model runner might keep GGUF
    /// files and model directories, so that a word like "qwen" does not also match a file in an image tool's
    /// folder: `library.only(|e| matches!(e.format, Format::Gguf | Format::HfDirectory))`. `only(|_| true)` keeps
    /// every model again. A path given to [`Library::resolve`] is taken whatever the rule says: the caller chose it.
    pub fn only(&mut self, rule: impl Fn(&Entry) -> bool + Send + Sync + 'static) {
        self.only = Some(Box::new(rule));
        self.entries = None;
    }

    /// Every model found (that [`Library::only`]'s rule keeps), scanning the locations the first time.
    pub fn entries(&mut self) -> &[Entry] {
        let only = &self.only;
        self.entries.get_or_insert_with(|| {
            let mut entries = scan(&self.locations.read);
            if let Some(rule) = only {
                entries.retain(|e| rule(e));
            }
            entries
        })
    }

    /// Forgets what was found, so that the next call scans again (after a program adds or removes a model).
    pub fn rescan(&mut self) {
        self.entries = None;
    }

    /// Every model whose id, file name or tags (keys and values) hold every word of `query`, ignoring case; every
    /// model for an empty query.
    ///
    /// DECISION: a search reads the tags only of models whose identity is already in the fingerprint cache, which
    /// is every model tagged or looked up on this machine, so that a search never reads models. A model copied in
    /// from elsewhere is found by its tags once anything has asked for its identity ([`Library::identity`]).
    pub fn search(&mut self, query: &str) -> Vec<Entry> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let entries = self.entries().to_vec();
        let tagged = self.store.all();
        let found = entries
            .into_iter()
            .filter(|entry| {
                let file = entry.path.file_name().unwrap_or_default().to_string_lossy();
                let mut text = format!("{} {file}", entry.id);
                if !tagged.is_empty()
                    && let Some(id) = self.known_identity(entry)
                    && let Some((_, owners, _)) = tagged.iter().find(|(i, _, _)| *i == id)
                {
                    text.push_str(&Tags::new(owners.clone(), None).text());
                }
                let text = text.to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect();
        self.cache.save();
        found
    }

    /// The model `name` names (see the module's comment for the order).
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing matches, [`Error::Ambiguous`] when a search matches more than one model.
    pub fn resolve(&mut self, name: &str) -> Result<Entry> {
        self.resolve_from(name, 0)
    }

    fn resolve_from(&mut self, name: &str, depth: usize) -> Result<Entry> {
        let not_found = |hint: &str| Error::NotFound { name: name.to_string(), hint: hint.to_string() };
        let path = Path::new(name);
        if path.exists() {
            return entry_for_path(path).ok_or_else(|| not_found(": it is neither a model file nor a model directory"));
        }
        if let Some(target) = self.locations.names.get(name).cloned() {
            if depth >= 4 {
                return Err(not_found(": its nickname leads to nicknames four deep"));
            }
            return self.resolve_from(&target, depth + 1);
        }
        let same = |e: &&Entry| e.id.eq_ignore_ascii_case(name);
        if let Some(entry) = self.entries().iter().filter(|e| e.hub.is_none()).find(same) {
            return Ok(entry.clone());
        }
        if let Some(hub) = HubRef::parse(name) {
            return self.cached(&hub).ok_or_else(|| not_found(": the Hugging Face cache does not hold it"));
        }
        if let Some(entry) = self.entries().iter().find(same) {
            return Ok(entry.clone());
        }
        let mut found = self.search(name);
        match found.len() {
            1 => Ok(found.remove(0)),
            0 => Err(not_found(": no model's id or tags hold every word")),
            _ => {
                Err(Error::Ambiguous { name: name.to_string(), candidates: found.into_iter().map(|e| e.id).collect() })
            }
        }
    }

    /// The cached model that `hub` names: its GGUF file by quant tag (`files::best_model`) among
    /// the files of the snapshot the revision names, or its model directory when it has no GGUF files and no tag
    /// was given.
    ///
    /// SPEC tenstag specs/stores/hf-hub-cache.md HFC-015 (IMPL): with no revision, the snapshot of refs/main
    /// (huggingface_hub), else of the first other ref (llama.cpp; by name here, its order being the directory's).
    /// HFC-013 (IMPL), offline: a revision of 40 hex digits is a commit, anything else is looked up in refs/.
    pub fn cached(&mut self, hub: &HubRef) -> Option<Entry> {
        let in_repo: Vec<Entry> = self
            .entries()
            .iter()
            .filter(|e| e.hub.as_ref().is_some_and(|h| h.repo.eq_ignore_ascii_case(&hub.repo)))
            .cloned()
            .collect();
        let commit = match hub.revision.as_deref() {
            Some(r) if r.len() == 40 && r.bytes().all(|b| b.is_ascii_hexdigit()) => Some(r.to_ascii_lowercase()),
            Some(r) => in_repo
                .iter()
                .filter_map(|e| e.hub.as_ref())
                .find(|h| h.refs.iter().any(|n| n == r))
                .map(|h| h.commit.clone()),
            None => {
                let origins: Vec<_> = in_repo.iter().filter_map(|e| e.hub.as_ref()).collect();
                let main = origins.iter().find(|h| h.refs.iter().any(|n| n == "main"));
                let first = || origins.iter().filter(|h| !h.refs.is_empty()).min_by_key(|h| h.refs[0].clone());
                main.or_else(first).map(|h| h.commit.clone())
            }
        };
        let commit = commit?;
        let candidates: Vec<Entry> =
            in_repo.into_iter().filter(|e| e.hub.as_ref().is_some_and(|h| h.commit == commit)).collect();
        let files: Vec<&str> = candidates
            .iter()
            .filter_map(|e| e.hub.as_ref().map(|h| h.file.as_str()))
            .filter(|f| !f.is_empty())
            .collect();
        if let Some(file) = best_model(files.iter().copied(), hub.quant.as_deref()) {
            let file = file.to_string();
            return candidates.into_iter().find(|e| e.hub.as_ref().is_some_and(|h| h.file == file));
        }
        // SPEC tenstag specs/names.md NAM-004 (MUST): with no quant and no GGUF file, the snapshot's folder entry,
        // whatever it is (a model folder, a pipeline, an adapter).
        if hub.quant.is_none() && files.is_empty() {
            return candidates.into_iter().find(|e| e.hub.as_ref().is_some_and(|h| h.file.is_empty()));
        }
        None
    }
}

impl Library {
    /// The identity of `entry`'s model: its fingerprint, from the fingerprint cache when the files have not changed
    /// (tenstag specs/fingerprint-cache.md TFC-003), else read (about 33 MiB of each file at most).
    ///
    /// # Errors
    ///
    /// [`Error::Tag`] when the model has no files to identify it by (a model a store keeps in a form tenstag does
    /// not list the files of, [`Format::Other`](crate::Format::Other)); [`Error::Io`] when a file cannot be read.
    pub fn identity(&mut self, entry: &Entry) -> Result<Identity> {
        let result = self.identity_unsaved(entry);
        self.cache.save();
        result
    }

    fn identity_unsaved(&mut self, entry: &Entry) -> Result<Identity> {
        let files = members(entry);
        let mut ids = Vec::with_capacity(files.len());
        for file in &files {
            ids.push(self.cache.file_identity(file)?);
        }
        set_identity(&ids).ok_or_else(|| Error::Tag(format!("{}: it has no files to identify it by", entry.id)))
    }

    /// The identity of `entry` when the cache has every one of its files, without reading any.
    fn known_identity(&mut self, entry: &Entry) -> Option<Identity> {
        let ids: Option<Vec<Identity>> = members(entry).iter().map(|f| self.cache.known(f)).collect();
        set_identity(&ids?)
    }

    /// The size of `entry`'s model as its tag file records it: its members' sizes together (TGS-003).
    fn model_bytes(entry: &Entry) -> u64 {
        members(entry).iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum()
    }

    /// `entry`'s tags: those of its identity, which every copy of the model shares, of every owner. `tags.value(key)`
    /// and the others read the program's own ([`Tags::mine`]); `tags.of(owner)` reads another program's.
    ///
    /// # Errors
    ///
    /// As for [`Library::identity`].
    pub fn tags(&mut self, entry: &Entry) -> Result<Tags> {
        let identity = self.identity(entry)?;
        Ok(Tags::new(self.store.tags(&identity), self.owner.clone()))
    }

    /// Sets the program's tag `tag` on `entry`'s model: a word (`chat`) or `key=value` (`arch=qwen3`). A tag whose
    /// key the program has already set replaces it, whatever its value: `arch=llama` replaces `arch=qwen3`. This call
    /// changes no other owner's tags.
    ///
    /// Tags are a cache (tenstag specs/tags.md TAG-008): keep in them what the program can work out again from the
    /// model, and work it out again when a tag is missing. Something only a person can say (a favourite) belongs in
    /// the program's own settings.
    ///
    /// # Errors
    ///
    /// [`Error::Tag`] when the library has no owner, when `tag` is not a tag (a key of 1 to 256 bytes with no space,
    /// `=` or control character; a value of 1 to 4,096 bytes with no control character), when the program would have
    /// more than 1,024 tags of the model, or when the model cannot be identified; [`Error::Io`] when the cache folder
    /// cannot be written.
    pub fn tag(&mut self, entry: &Entry, tag: &str) -> Result<()> {
        let (owner, version) = self.me("tag")?;
        parse_tag(tag)?;
        let identity = self.identity(entry)?;
        let seen = std::fs::canonicalize(&entry.path).ok().map(|p| path_text(&p));
        let sha256 = self.known_sha256(entry);
        let sha256 = sha256.as_ref().map(|(h, f)| (h.as_str(), f.as_str()));
        let bytes = Library::model_bytes(entry);
        self.store.change(&identity, &owner, version, bytes, seen.as_deref(), sha256, |tags| apply(tags, tag))
    }

    /// Removes the program's key `key` from `entry`'s model's tags, whatever its value. Removing a key it does not
    /// have does nothing.
    ///
    /// # Errors
    ///
    /// [`Error::Tag`] when the library has no owner, or `key` is not a key (it holds `=`, a space or a control
    /// character); otherwise as for [`Library::tag`].
    pub fn untag(&mut self, entry: &Entry, key: &str) -> Result<()> {
        let (owner, version) = self.me("untag")?;
        if parse_tag(key)?.1.is_some() {
            return Err(Error::Tag(format!("{key:?}: untag takes a key, not a key and a value")));
        }
        let identity = self.identity(entry)?;
        if !Tags::new(self.store.tags(&identity), self.owner.clone()).has(key) {
            return Ok(());
        }
        let seen = std::fs::canonicalize(&entry.path).ok().map(|p| path_text(&p));
        self.store.change(&identity, &owner, version, Library::model_bytes(entry), seen.as_deref(), None, |tags| {
            tags.remove(key);
            Ok(())
        })
    }

    /// Removes every one of the program's tags of `entry`'s model, and no other program's.
    ///
    /// # Errors
    ///
    /// As for [`Library::untag`].
    pub fn untag_all(&mut self, entry: &Entry) -> Result<()> {
        let (owner, version) = self.me("untag_all")?;
        let identity = self.identity(entry)?;
        if !self.store.tags(&identity).contains_key(&owner) {
            return Ok(());
        }
        self.store.change(&identity, &owner, version, Library::model_bytes(entry), None, None, |tags| {
            tags.clear();
            Ok(())
        })
    }

    /// Every model the program has tagged `query`: a key (`arch`: with any value or none) or `key=value`
    /// (`arch=qwen3`: exactly that value), among its own tags at its version. Matching is exact, letter case
    /// included.
    ///
    /// Only models of the size a matching tag file records are identified, so a query no tag matches reads nothing,
    /// and one that matches reads only the models that may be the ones tagged.
    ///
    /// # Errors
    ///
    /// [`Error::Tag`] when the library has no owner, or `query` is not a tag. A model that cannot be read is left
    /// out, not an error.
    pub fn tagged(&mut self, query: &str) -> Result<Vec<Entry>> {
        self.me("tagged")?;
        let (key, value) = parse_tag(query)?;
        Ok(self.matching(|tags| tags.mine().matches(&key, value.as_deref())))
    }

    /// Every model whose value of the program's `key` holds the item `item`: `tagged_with("needs", "t5xxl")` finds
    /// `needs=t5xxl,clip-l` (tenstag specs/tags.md TAG-006: the value split at `,`, each part trimmed of spaces).
    ///
    /// # Errors
    ///
    /// [`Error::Tag`] when the library has no owner, or `key` is not a key.
    pub fn tagged_with(&mut self, key: &str, item: &str) -> Result<Vec<Entry>> {
        self.me("tagged_with")?;
        if parse_tag(key)?.1.is_some() {
            return Err(Error::Tag(format!("{key:?}: tagged_with takes a key, not a key and a value")));
        }
        Ok(self.matching(|tags| tags.mine().value_set(key).contains(item)))
    }

    /// Every model another program, `owner`, has tagged `query` (as [`Library::tagged`], among that owner's tags,
    /// whatever their version). Needs no owner of its own.
    ///
    /// # Errors
    ///
    /// [`Error::Tag`] when `query` is not a tag.
    pub fn tagged_by(&mut self, owner: &str, query: &str) -> Result<Vec<Entry>> {
        let (key, value) = parse_tag(query)?;
        Ok(self.matching(|tags| tags.of(owner).matches(&key, value.as_deref())))
    }

    /// Every model whose tags `test` accepts: for any other question, across owners
    /// (`|tags| tags.of("imagenode").value_set("needs").contains("t5xxl")`).
    pub fn tagged_where(&mut self, test: impl Fn(&Tags) -> bool) -> Vec<Entry> {
        self.matching(test)
    }

    /// The models whose tags `test` accepts.
    fn matching(&mut self, test: impl Fn(&Tags) -> bool) -> Vec<Entry> {
        let me = self.owner.clone();
        let wanted: Vec<(Identity, u64)> = self
            .store
            .all()
            .into_iter()
            .filter(|(_, owners, _)| test(&Tags::new(owners.clone(), me.clone())))
            .map(|(id, _, bytes)| (id, bytes))
            .collect();
        if wanted.is_empty() {
            return Vec::new();
        }
        let entries = self.entries().to_vec();
        let mut found = Vec::new();
        for entry in entries {
            let bytes = Library::model_bytes(&entry);
            if !wanted.iter().any(|(_, b)| *b == 0 || *b == bytes) {
                continue;
            }
            match self.identity_unsaved(&entry) {
                Ok(id) if wanted.iter().any(|(w, _)| *w == id) => found.push(entry),
                Ok(_) => {}
                Err(e) => log::debug!("{}: not identified ({e})", entry.id),
            }
        }
        self.cache.save();
        found
    }

    /// The identity `entry`'s file had before its contents last changed, and that identity's tags, when the
    /// fingerprint cache remembers one: a program may carry tags over to the new contents ([`Library::tag`]).
    /// Only a model of one file has one.
    ///
    /// # Errors
    ///
    /// As for [`Library::identity`].
    pub fn previous(&mut self, entry: &Entry) -> Result<Option<(Identity, Tags)>> {
        self.identity(entry)?;
        let files = members(entry);
        let [file] = files.as_slice() else { return Ok(None) };
        Ok(self.cache.previous(file).map(|id| {
            let tags = Tags::new(self.store.tags(&id), self.owner.clone());
            (id, tags)
        }))
    }

    /// The full SHA-256 of `entry`'s model, a model of one file, reading every byte (`progress` is told the bytes
    /// read so far and the size). It is recorded in the model's tag file, when it has one, for other programs.
    ///
    /// # Errors
    ///
    /// [`Error::Tag`] for a model of more than one file; [`Error::Io`] when the file cannot be read.
    pub fn verify(&mut self, entry: &Entry, progress: &mut dyn FnMut(u64, u64)) -> Result<String> {
        let files = members(entry);
        let [file] = files.as_slice() else {
            return Err(Error::Tag(format!("{}: a SHA-256 is of a model of one file", entry.id)));
        };
        let identity = self.identity(entry)?;
        let sha256 = sha256_file(file, progress)?;
        self.cache.set_read_sha256(file, &sha256);
        self.cache.save();
        self.store.record_sha256(&identity, &sha256, "read")?;
        Ok(sha256)
    }

    /// The SHA-256 of `entry`'s model, a model of one file, when it is known without reading the file: the name its
    /// store gives it (the Hugging Face cache's LFS blobs, Ollama's and Docker Model Runner's blobs; tenstag
    /// specs/fingerprint-cache.md TFC-006), or an earlier [`Library::verify`]. `None` otherwise, and for a model of
    /// several files.
    ///
    /// A Hugging Face cache name is what the Hub said the file is; the cache does not check it. `verify` checks.
    ///
    /// # Errors
    ///
    /// As for [`Library::identity`] (the file's fingerprint is looked up, or computed, first).
    pub fn sha256(&mut self, entry: &Entry) -> Result<Option<String>> {
        self.identity(entry)?;
        Ok(self.known_sha256(entry).map(|(h, _)| h))
    }

    /// The SHA-256 the cache knows for a one-file model, with where it came from.
    fn known_sha256(&mut self, entry: &Entry) -> Option<(String, String)> {
        let files = members(entry);
        let [file] = files.as_slice() else { return None };
        self.cache.sha256(file)
    }

    /// Cleans the tag folder and the fingerprint cache, and returns how many items it removed: temporary files left
    /// over an hour, and the cache rows of files gone from folders that are still there. No tags are ever removed:
    /// a model not found may be on a drive that is not mounted.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the fingerprint cache cannot be written.
    pub fn clean_tags(&mut self) -> Result<usize> {
        let rows = self.cache.clean().map_err(|e| Error::io(&self.locations.cache, e))?;
        Ok(rows + self.store.clean())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Environment;
    use crate::locations::Location;
    use crate::scan::Format;
    use crate::scan::tests::{hub_snapshot, write};
    use crate::test_dir;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, std::path::PathBuf)]) -> Environment {
        let map: HashMap<String, String> =
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string_lossy().to_string())).collect();
        Environment::from_map(map)
    }

    fn library(root: &Path, config: &str) -> Library {
        std::fs::write(root.join("config.toml"), config).unwrap();
        owned(Library::new(
            Locations::load_from(vars(&[
                ("HOME", root.join("home")),
                ("HF_HUB_CACHE", root.join("hub")),
                ("TENSTAG_CONFIG", root.join("config.toml")),
                ("TENSTAG_PATH", root.join("models")),
                ("TENSTAG_CACHE_DIR", root.join("cache")),
            ]))
            .unwrap(),
        ))
    }

    /// A library owning its tags as `test`, version 1.
    fn owned(mut library: Library) -> Library {
        library.set_owner("test", 1).unwrap();
        library
    }

    #[test]
    fn hub_references_are_read_as_llama_cpp_reads_them() {
        let r = HubRef::parse("hf.co/Qwen/Qwen3-0.6B-GGUF:q8_0@main").unwrap();
        assert_eq!(
            (r.repo.as_str(), r.quant.as_deref(), r.revision.as_deref()),
            ("Qwen/Qwen3-0.6B-GGUF", Some("q8_0"), Some("main"))
        );
        assert_eq!(r.to_string(), "Qwen/Qwen3-0.6B-GGUF:q8_0@main");
        for bad in ["qwen3", "a/b/c", "/abs", "a/.b", "a/b-", "a/b:", "a//b", "a/b@", "a/b:x/y.gguf"] {
            assert_eq!(HubRef::parse(bad), None, "{bad}");
        }
        assert_eq!(HubRef::parse("o/m:v1.5").unwrap().quant.as_deref(), Some("v1.5"));
    }

    #[test]
    fn a_program_gives_its_own_folders_whatever_the_configuration_says() {
        let root = test_dir("program");
        write(&root.join("app/models/deep/er/m-Q8_0.gguf"), b"12345");
        write(&root.join("app/images/unet/flux-Q4_K_S.gguf"), b"1");
        // A configuration the program cannot read, and a user who turned the defaults off.
        let config = root.join("config.toml");
        std::fs::write(&config, "defaults = false\ncolour = \"broken\"\n").unwrap();
        let env = vars(&[
            ("HOME", root.join("home")),
            ("TENSTAG_CONFIG", config.clone()),
            ("TENSTAG_CACHE_DIR", root.join("cache")),
        ]);
        assert!(Locations::load_from(env.clone()).is_err());
        let mut locations = Locations::bare_from(env);
        assert!(locations.read.is_empty());
        assert_eq!(locations.cache, root.join("cache"), "the tags stay where every program keeps them");
        locations.add(Location::folder(root.join("app/models")).depth(3));
        let images = root.join("app/images");
        locations.add(Location::folder(&images).what("the app's images"));
        let mut lib = owned(Library::new(locations));
        lib.only(move |e| !e.path.starts_with(&images));
        let entry = lib.resolve("m-Q8_0").unwrap();
        assert_eq!(entry.id, "deep/er/m-Q8_0");
        assert!(lib.search("flux").is_empty(), "the program's rule leaves its image folder out");
        lib.tag(&entry, "layers=2 layers").unwrap();
        assert_eq!(lib.tags(&entry).unwrap().value("layers"), Some("2 layers"));
        assert!(lib.locations().describe().iter().any(|l| l.contains("the app's images [program]")));

        // Too shallow a depth: the folder is read only as deep as the program says.
        let mut shallow = Locations::bare_from(Environment::from_map(Default::default()));
        shallow.add(Location::folder(root.join("app/models")).depth(2));
        assert!(Library::new(shallow).entries().is_empty());
    }

    #[test]
    fn names_resolve_in_order_and_searches_are_never_guessed() {
        let root = test_dir("names");
        write(&root.join("models/qwen3-0.6b-q8_0.gguf"), b"1");
        write(&root.join("models/llama-3.2-1b-q4_k_m.gguf"), b"1");
        write(&root.join("models/llama-3.2-3b-q4_k_m.gguf"), b"1");
        write(&root.join("models/dirs/tiny/config.json"), b"{}");
        write(&root.join("models/dirs/tiny/model.safetensors"), b"1");
        hub_snapshot(
            &root.join("hub"),
            "Qwen/Qwen3-0.6B-GGUF",
            &"a".repeat(40),
            &["main"],
            &[("Qwen3-0.6B-Q8_0.gguf", b"1")],
        );
        let mut lib = library(&root, "[names]\nq = \"Qwen/Qwen3-0.6B-GGUF:q8_0\"\n");

        assert_eq!(lib.resolve("q").unwrap().id, "Qwen/Qwen3-0.6B-GGUF:Q8_0");
        assert!(matches!(lib.resolve("Qwen/Qwen3-4B@v1"), Err(Error::NotFound { .. })), "nothing is downloaded");
        // A model of another store whose id looks like a repository is that model.
        assert_eq!(lib.resolve("dirs/tiny").unwrap().id, "dirs/tiny");
        assert_eq!(lib.resolve("QWEN3-0.6b-q8_0").unwrap().id, "qwen3-0.6b-q8_0");
        assert_eq!(lib.resolve("3.2 3b").unwrap().id, "llama-3.2-3b-q4_k_m");
        match lib.resolve("llama q4") {
            Err(Error::Ambiguous { candidates, .. }) => assert_eq!(candidates.len(), 2),
            other => panic!("{other:?}"),
        }
        assert!(matches!(lib.resolve("mistral"), Err(Error::NotFound { .. })));
        let file = root.join("models/qwen3-0.6b-q8_0.gguf");
        assert_eq!(lib.resolve(file.to_str().unwrap()).unwrap().id, file.to_str().unwrap().trim_end_matches(".gguf"));
    }

    #[test]
    fn the_cache_alone_is_chosen_from_as_huggingface_hub_and_llama_cpp_choose() {
        let root = test_dir("cached");
        let (main, v1, loose) = ("a".repeat(40), "b".repeat(40), "c".repeat(40));
        let hub = root.join("hub");
        hub_snapshot(&hub, "Org/M-GGUF", &main, &["main"], &[("M-Q8_0.gguf", b"1"), ("M-F16.gguf", b"1")]);
        hub_snapshot(&hub, "Org/M-GGUF", &v1, &["v1", "bbbbbbb"], &[("M-Q4_K_M.gguf", b"1")]);
        hub_snapshot(&hub, "Org/M-GGUF", &loose, &[], &[("M-Q4_0.gguf", b"1")]);
        hub_snapshot(&hub, "Org/N-GGUF", &v1, &["dev"], &[("N-Q8_0.gguf", b"1")]);
        let mut lib = library(&root, "");
        let mut file = |name: &str| lib.cached(&HubRef::parse(name).unwrap()).map(|e| e.hub.unwrap().file);
        // HFC-015: refs/main's snapshot (Q4_K_M is in another), then Q8_0 by HFC-010's default order.
        assert_eq!(file("Org/M-GGUF").as_deref(), Some("M-Q8_0.gguf"));
        assert_eq!(file("Org/M-GGUF:Q4_K_M"), None);
        // HFC-013 offline: a ref by name, a short commit only as a ref, a full commit as itself.
        assert_eq!(file("Org/M-GGUF@v1").as_deref(), Some("M-Q4_K_M.gguf"));
        assert_eq!(file("Org/M-GGUF@bbbbbbb").as_deref(), Some("M-Q4_K_M.gguf"));
        assert_eq!(file("Org/M-GGUF@ccccccc"), None);
        assert_eq!(file(&format!("Org/M-GGUF@{loose}")).as_deref(), Some("M-Q4_0.gguf"));
        // Without main, the first other ref (llama.cpp).
        assert_eq!(file("Org/N-GGUF").as_deref(), Some("N-Q8_0.gguf"));
    }

    #[test]
    fn tags_follow_a_model_wherever_it_goes() {
        let root = test_dir("tagging");
        write(&root.join("models/a-Q8_0.gguf"), b"model a");
        write(&root.join("models/b-Q4_0.gguf"), b"model b, larger");
        write(&root.join("models/tiny/config.json"), b"{}");
        write(&root.join("models/tiny/model.safetensors"), b"weights");
        write(&root.join("models/tiny/README.md"), b"a card");
        let mut lib = library(&root, "");
        let a = lib.resolve("a-Q8_0").unwrap();
        lib.tag(&a, "favourite").unwrap();
        lib.tag(&a, "lang=en, fr").unwrap();
        assert!(lib.tag(&a, "two words").is_err());
        assert!(lib.untag(&a, "lang=en").is_err(), "untag takes a key");
        let tags = lib.tags(&a).unwrap();
        assert_eq!(tags.value("lang"), Some("en, fr"));
        assert!(tags.value_set("lang").contains("fr"));

        let ids = |found: Vec<Entry>| found.into_iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids(lib.tagged("lang").unwrap()), ["a-Q8_0"]);
        assert_eq!(ids(lib.tagged("lang=en, fr").unwrap()), ["a-Q8_0"]);
        assert!(lib.tagged("lang=en").unwrap().is_empty(), "a value matches exactly");
        assert_eq!(ids(lib.tagged_with("lang", "en").unwrap()), ["a-Q8_0"]);
        assert_eq!(ids(lib.search("favourite")), ["a-Q8_0"]);
        assert_eq!(lib.resolve("favourite").unwrap().id, "a-Q8_0", "a search reads the tags");

        // Setting a key replaces it.
        lib.tag(&a, "lang=de").unwrap();
        assert!(lib.tagged_with("lang", "en").unwrap().is_empty());

        // Moved and renamed: the same model, the same tags, in another program.
        std::fs::create_dir_all(root.join("models/moved")).unwrap();
        std::fs::rename(root.join("models/a-Q8_0.gguf"), root.join("models/moved/renamed.gguf")).unwrap();
        let mut other = library(&root, "");
        let moved = other.resolve("renamed").unwrap();
        assert_eq!(other.tags(&moved).unwrap().value("lang"), Some("de"));
        assert_eq!(ids(other.tagged("favourite").unwrap()), ["moved/renamed"]);

        // A model directory: its model card is not part of it.
        let tiny = other.resolve("dirs/tiny").or_else(|_| other.resolve("tiny")).unwrap();
        let before = other.identity(&tiny).unwrap();
        std::fs::write(root.join("models/tiny/README.md"), b"a longer card").unwrap();
        assert_eq!(other.identity(&tiny).unwrap(), before);
        other.untag(&moved, "favourite").unwrap();
        other.untag(&moved, "lang").unwrap();
        assert!(other.tags(&moved).unwrap().is_empty());
        assert!(other.tagged("lang").unwrap().is_empty());

        // Changed contents: a new identity, with the old one and its tags kept for the program to carry over.
        let b = other.resolve("b-Q4_0").unwrap();
        other.tag(&b, "kept").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&b.path, b"model b, edited").unwrap();
        assert!(other.tags(&b).unwrap().is_empty());
        let (_, old) = other.previous(&b).unwrap().expect("the identity before");
        assert!(old.has("kept"));
        assert_eq!(other.sha256(&b).unwrap(), None, "a plain file's name says nothing");
        let sha = other.verify(&b, &mut |_, _| {}).unwrap();
        assert_eq!(other.sha256(&b).unwrap(), Some(sha.clone()), "remembered once read");
        assert_eq!(sha.len(), 64);
    }

    #[cfg(unix)]
    #[test]
    fn a_hugging_face_blob_gives_its_sha256_without_reading_it() {
        use sha2::{Digest, Sha256};
        let root = test_dir("store-sha256");
        let bytes = b"a model held in LFS";
        let hex: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
        let repo = root.join("hub/models--org--m-GGUF");
        let commit = "e".repeat(40);
        write(&repo.join("blobs").join(&hex), bytes);
        write(&repo.join("refs/main"), commit.as_bytes());
        std::fs::create_dir_all(repo.join("snapshots").join(&commit)).unwrap();
        std::os::unix::fs::symlink(
            format!("../../blobs/{hex}"),
            repo.join("snapshots").join(&commit).join("m-Q8_0.gguf"),
        )
        .unwrap();
        let mut lib = library(&root, "");
        let entry = lib.resolve("org/m-GGUF:Q8_0").unwrap();
        assert_eq!(lib.sha256(&entry).unwrap(), Some(hex.clone()));
        lib.tag(&entry, "favourite").unwrap();
        let identity = lib.identity(&entry).unwrap();
        let file: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("cache/tags/tfp1").join(format!("{}.json", &identity.as_str()[5..])))
                .unwrap(),
        )
        .unwrap();
        assert_eq!((file["sha256"].as_str(), file["sha256_from"].as_str()), (Some(hex.as_str()), Some("store")));
        // Read from the bytes: the same here, and now a checked fact.
        assert_eq!(lib.verify(&entry, &mut |_, _| {}).unwrap(), hex);
        let file: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("cache/tags/tfp1").join(format!("{}.json", &identity.as_str()[5..])))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(file["sha256_from"], "read");
    }

    #[test]
    fn a_program_chooses_the_models_it_works_with_by_its_own_rule() {
        let root = test_dir("only");
        write(&root.join("models/qwen3-0.6b-Q8_0.gguf"), b"12345");
        write(&root.join("models/comfy/text_encoders/qwen-image-encoder.safetensors"), &vec![0u8; 1 << 20]);
        let mut lib = library(&root, "");
        assert!(matches!(lib.resolve("qwen"), Err(Error::Ambiguous { .. })));
        lib.only(|e| e.format == Format::Gguf);
        assert_eq!(lib.resolve("qwen").unwrap().id, "qwen3-0.6b-Q8_0");
        let encoder = root.join("models/comfy/text_encoders/qwen-image-encoder.safetensors");
        assert!(lib.resolve(encoder.to_str().unwrap()).is_ok(), "a path is taken whatever the rule says");
        lib.only(|_| true);
        assert_eq!(lib.entries().len(), 2);
    }

    #[test]
    fn programs_write_only_their_own_tags_and_a_new_version_starts_afresh() {
        let root = test_dir("owners");
        write(&root.join("models/m-Q8_0.gguf"), b"a model");
        let mut chatrun = library(&root, "");
        chatrun.set_owner("chatrun", 1).unwrap();
        let mut imagenode = library(&root, "");
        imagenode.set_owner("imagenode", 1).unwrap();
        let m = chatrun.resolve("m-Q8_0").unwrap();
        chatrun.tag(&m, "arch=qwen3").unwrap();
        imagenode.tag(&m, "arch=unet").unwrap();
        imagenode.tag(&m, "needs=t5xxl,clip-l").unwrap();

        let tags = chatrun.tags(&m).unwrap();
        assert_eq!(tags.value("arch"), Some("qwen3"), "its own");
        assert_eq!(tags.of("imagenode").value("arch"), Some("unet"), "another's, by name");
        assert_eq!(tags.owners().map(|(o, _)| o).collect::<Vec<_>>(), ["chatrun", "imagenode"]);
        let ids = |found: Vec<Entry>| found.into_iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids(chatrun.tagged("arch=qwen3").unwrap()), ["m-Q8_0"]);
        assert!(chatrun.tagged("arch=unet").unwrap().is_empty(), "its own tags only");
        assert_eq!(ids(chatrun.tagged_by("imagenode", "arch=unet").unwrap()), ["m-Q8_0"]);
        assert_eq!(ids(chatrun.tagged_where(|t| t.of("imagenode").value_set("needs").contains("t5xxl"))), ["m-Q8_0"]);
        assert_eq!(ids(chatrun.search("unet")), ["m-Q8_0"], "a search reads every owner's tags");

        // chatrun's untag_all leaves imagenode's tags.
        chatrun.untag_all(&m).unwrap();
        assert!(chatrun.tags(&m).unwrap().mine().is_empty());
        assert_eq!(imagenode.tags(&m).unwrap().value("arch"), Some("unet"));

        // imagenode at version 2: its version-1 tags count as missing, and its first write replaces them.
        imagenode.set_owner("imagenode", 2).unwrap();
        let tags = imagenode.tags(&m).unwrap();
        assert!(!tags.has("arch"), "written at another version");
        assert_eq!(tags.of("imagenode").version(), 1);
        imagenode.tag(&m, "family=flux1").unwrap();
        let tags = imagenode.tags(&m).unwrap();
        assert_eq!(tags.mine().iter().collect::<Vec<_>>(), [("family", Some("flux1"))]);

        // A library with no owner reads, and writes nothing.
        let mut reader = Library::new(chatrun.locations().clone());
        assert_eq!(reader.tags(&m).unwrap().of("imagenode").value("family"), Some("flux1"));
        assert!(matches!(reader.tag(&m, "x"), Err(Error::Tag(_))));
        assert!(matches!(reader.tagged("x"), Err(Error::Tag(_))));
        assert!(Library::new(chatrun.locations().clone()).set_owner("Chat Run", 1).is_err());
    }
}
