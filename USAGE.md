# Using tenstag

Every call tenstag offers and what it promises. README.md introduces it with examples; this is the reference.
Anything described here changes only with a note in CHANGELOG.md, and while the version is 0.x a change that could
break a program takes a new minor version (0.2 -> 0.3), so a program pinned to a 0.2 tag keeps working across 0.2.x.

## What tenstag does and does not do

- It **finds** models in well-known places (the Hugging Face cache, llama.cpp's cache, LM Studio's folder,
  Ollama's store, configured directories), **names** them, and turns a name into a model: language models, and
  image and video models (diffusers pipelines, single checkpoint and LoRA files). It reads the layouts of stores that hide their models behind
  their own names (hashed blobs, manifests); a folder of plain files, such as an image tool's models folder, is
  listed file by file, and what its subfolders mean is left to the program using it.
- It **does not say what kind of model** a file is (a language model, an image model, a text encoder). It gives
  facts: the format, the path, the id, the location. The program decides, and may record its decision as a tag.
- It keeps a **cache of what programs worked out** about models, as tags, which programs set, remove and find
  models by. Tags belong to a model's *identity*, a fingerprint of its contents, so a model moved, renamed or copied
  keeps them.
- It **does not interpret model data.** It reads a model's bytes only to fingerprint it (at most about 33 MiB of
  each file, once: the fingerprint is cached) or, when a program asks, to compute a full SHA-256; and the four-byte
  GGUF signature of a blob in Ollama's store (tenstag specs/stores/ollama.md SUG-041). Otherwise it reads only
  the stores' own bookkeeping: manifests, refs, settings files, a safetensors index's list of shards.
- It **writes only** its cache folder (fingerprints and tags), and the configuration file when a program asks it to
  add or remove a location or set `defaults`. Every location is read-only; no model is changed.
- It **uses no network** and downloads nothing.

## The calls

```text
Locations::load()                    where to look, from the configuration file, the environment and the defaults
Locations::load_from(Environment)    (the second for tests and embedding: a map, not the process's)
Locations::bare()                    no locations: no configuration file, no default locations
Locations::bare_from(Environment)
locations.add(location)              a location the program gives (read after the others)
locations.cache                      the cache folder: fingerprints and tags (a program may set its own)
Location::new(path, kind)            a location as a program gives it; Location::folder(path) for Kind::Directory;
  .depth(n) .min_size(b) .what(text)   its settings, as the configuration's keys of the same names
locations.describe()                 one line per location, for a person
locations.default_locations()        the built-in locations, even when `defaults = false` (what turning them on adds)

locations.config_path()              the configuration file a change is written to, whether or not it exists yet
locations.add_to_config(&location, added_by)  -> AddedToConfig::Added(location) | AlreadyListed(location)
                                     a [[location]] added to the file, with added_by (an owner name) and the time now
locations.remove_from_config(path)   -> Option<Location>   every [[location]] of that folder removed; the first given
locations.set_defaults(on)           -> bool               `defaults` set in the file; whether it changed

Library::open()                      Locations::load() and a library over them, reading tags only; nothing is scanned yet
Library::open_as(owner, version)     the same, for the program that owns its tags as owner, at version
Library::new(locations)
library.set_owner(owner, version)    the tag owner from now on (com.example.myapp, or a GitHub user/repository)
library.only(|e| rule)               from now on, list, search, tag-query and resolve among the models the program's
                                     rule keeps (all by default; only(|_| true) to go back)
library.entries()                    every model found (scanned once; library.rescan() to scan again)
library.resolve(name)                -> Entry               (names, below)
library.cached(&hub_ref)             -> Option<Entry>       the Hugging Face cache's choice for a repository name
library.search(words)                -> Vec<Entry>          every word in the id, file name or tags

library.identity(&entry)             -> Identity            tfp1:<64 hex>, the model's fingerprint (tags, below)
library.tags(&entry)                 -> Tags                every owner's: .value(key) .has(key) .value_set(key) are
                                                            the program's own (.mine()); .of(owner) another's;
                                                            .owners(); OwnerTags: .version() .words() .iter()
library.tag(&entry, "key=value")     sets the program's word or key's value, replacing the key's value if it had one
library.untag(&entry, "key")         removes the program's key, whatever its value
library.untag_all(&entry)            removes all the program's tags of the model, never another's
library.tagged("key")                -> Vec<Entry>          the program's models with the key; "key=value": exactly
library.tagged_with("key", "item")   -> Vec<Entry>          the program's models whose value of key holds item
library.tagged_by(owner, "key=value") -> Vec<Entry>         another owner's, as tagged
library.tagged_where(|tags| test)    -> Vec<Entry>          any other question, across owners
library.previous(&entry)             -> Option<(Identity, Tags)>   the identity before the file last changed
library.verify(&entry, progress)     -> String              a full SHA-256, of a model of one file, read now
library.sha256(&entry)               -> Option<String>      the SHA-256 known without reading: the store's name
                                                            for the file, or an earlier verify
library.clean_tags()                 -> usize               removes leftover temporary files and stale cache rows

HubRef::parse("org/name[:quant][@revision]")   (hf.co/ in front allowed)
parse_tag("key=value")               -> (key, Option<value>), or Error::Tag saying why it is not a tag
value_set("en, fr")                  -> {"en", "fr"}
is_owner("chatrun")                  -> bool                whether a name is an owner name
fingerprint_file(path)               -> Identity            one file's fingerprint, read now, without the cache
entry.files()                        every file of the model, from names and listings only
```

An `Entry` has: `id` (unique among the entries), `path` (what a reader opens: a model file, the first part of a split
one, or a model directory), `format`, `quant` (a quant tag, or empty), `bytes`, `location`, `hub`
(repository, commit, refs, file, for the Hugging Face cache), `parts` (a split GGUF model whose parts have no part
names: Ollama's blobs) and `note` (what an `Other` model is).

`format` is how the model is stored. More may be added in a minor version, so a `match` on it needs a `_` arm:

| format | what `path` is |
|---|---|
| `Gguf` | a GGUF file, or a split set's first part |
| `HfDirectory` | a Hugging Face model directory: `config.json` and its weights |
| `Safetensors` | a single `.safetensors` (or `.sft`) file, or a split set's first part (`m-00001-of-00003.safetensors`) |
| `Checkpoint` | a single PyTorch checkpoint: `.ckpt` (and `.pt`, `.pth`, `.bin` given by path). A Python pickle: loading one runs code it holds |
| `Diffusers` | a diffusers pipeline directory: `model_index.json` and a folder per component |
| `Adapter` | an adapter directory as PEFT writes it: `adapter_config.json` and its weights |
| `Other` | a model a store holds in another form (`note` says what) |

`format` is a fact from names and layout, not what the model is for: a GGUF file may be a language model, an image
tool's quantised UNet, or a text encoder (an image tool's `text_encoders/` holds T5 and CLIP encoders, and some full
language models such as Qwen2.5-VL that a language-model runner could open). Only a program that reads the file can
tell, and tenstag never does.

A program that works with some models only says so once with its own rule, so that a name or a search does not also
match the rest: `library.only(|e| matches!(e.format, Format::Gguf | Format::HfDirectory))`, or a rule on the
folder (`e.location`, `e.path`), or one on what it tagged.

Errors (`tenstag::Error`): `Io`, `Config` (the configuration file), `NotFound`, `Ambiguous` (a search matching
more than one model, with the candidates), `Tag` (a tag or key that is not one, or a model that cannot be tagged).
Every message is written for the person running the program.

## Names

`Library::resolve` tries, in order: an existing path; a nickname from the configuration (`[names]`); the exact id of
a model not from the Hugging Face cache; a Hugging Face repository `org/name[:quant][@revision]`, which is the model
the cache holds, chosen as huggingface_hub and llama.cpp choose (`Library::cached`; `Error::NotFound` when the cache
does not hold it); the exact id of any model; a search, where every word must match. More than one match is
`Error::Ambiguous`.

## Tags

Tags are a cache of what programs worked out about models (tenstag specs/tags.md TAG-008). They are kept in the cache
folder, and a person may delete it at any time, so a program keeps in tags only what it can work out again from the
model, its formats and its store, and works out again what it finds missing. Something only a person can say, such
as a favourite, belongs in the program's own settings; a program may copy it into a tag for others to read, and
writes it again when it is missing.

Every tag has an owner, the program that wrote it (TAG-009): a name of 1 to 128 lowercase ASCII letters, digits, `-`,
`_`, `.` and `/`, starting with a letter or a digit, with a `/` only between two other characters. It must be one no
other program will choose (TAG-011): a reverse domain name its maker controls, then the program
(`com.example.chatrun`), or its GitHub repository in lower case (`example/chatrun`). It is given with a version (a `u32`,
TAG-010) by `Library::open_as` or `set_owner`. A program writes, changes and removes only its own tags; it reads its
own with `tags.value(key)` and the like, and any other owner's with `tags.of(owner)`. Its own tags written at another
version count as missing, and its first write at its version replaces them: raise the version when the way the
program works out its tags changes. Owners are not a protection (TAG-012): nothing checks an owner name, and the
tag files are plain files any program running as the person can change, so a program treats the tags it reads, its
own included, as unchecked input, and works out again a tag whose value makes no sense to it. A library without an
owner reads every owner's tags, and `tag`, `untag`,
`untag_all`, `tagged` and `tagged_with` are `Error::Tag`. `search` reads every owner's tags.

A tag is a word or a key and a value split at the first `=` (`arch=qwen3`; `a=b=c` is the key `a` and the value
`b=c`). A key is 1 to 256 bytes with no space, `=` or control character; a value is 1 to 4,096 bytes
with no control character, and is kept exactly as written. `key=` is not a tag. Keys and values compare exactly,
letter case included; `search` alone ignores case.

An owner's tags of a model are a map: each key once. `tag` with a key the owner has set replaces it, whatever either
was (`k=b` replaces `k=a`; `k` replaces `k=b`). Several items go in one value, and `value_set` reads them: the value
split at `,`, trimmed of spaces, without empty items. An owner has at most 1,024 tags of a model.

Tags belong to the model's identity, `tfp1:<64 hex>`: a fingerprint of its size, its first 16 MiB, its last 1 MiB
and 4,096 blocks of 4 KiB spread evenly between them (a file up to 33 MiB is read whole). A model of several files
(a split set, a model directory, a pipeline) is identified by its files' fingerprints together, leaving out `*.md`,
`LICENSE*`, `LICENCE*` and hidden files. So:
- the same model in two stores, or moved, renamed or copied, has the same identity and the same tags;
- a model whose bytes change has a new identity and no tags, and the program works them out again; `previous` gives
  the identity it had and that identity's tags (a model of one file);
- an edit in place that keeps the size and falls only between the sampled blocks is not seen (tenstag
  specs/fingerprint.md says how unlikely); `verify` computes a full SHA-256 when every byte matters.

A model of one file may have a SHA-256 known for free, because its store names the file by it: a Hugging Face cache
LFS blob (what the Hub said the file is: the cache does not check it), an Ollama blob, or a Docker Model Runner
`sha256` blob. RamaLama's blob names are not taken: some are hashes of names. `sha256` gives it, or `verify`'s
earlier result, and both are written to the model's tag file (`sha256_from` says `store` or `read`).

Fingerprints are cached by path, size and modification time, and on Unix by device and inode too, so a model is
read once, and a model moved on the same file system is not read again. `search` reads the tags of models whose
identity is already cached, so it never reads models; `tagged` and `tagged_with` identify only the models whose size
matches a tagged one.

The tag files and the cache are plain JSON, specified in tenstag/tenstag (specs/tag-store.md,
specs/fingerprint-cache.md) so that programs in other languages share them. Each identity with tags has one file;
writes are locked, and a file written by a later format is never changed. `clean_tags` never deletes tags: a model
not found may be on a drive that is not mounted.

## A program's own folders

A program may give locations itself, which need not be named in any configuration: `Locations::bare()` (no
configuration file, so a broken one is no error; no default locations; no TENSTAG_PATH) or `Locations::load()`,
then `locations.add(Location::folder(path).depth(n))`. Its models are found, named, searched, described and tagged
like any others (`Source::Program`), and their tags go in the usual cache folder (TENSTAG_CACHE_DIR, else the
default), so other programs see them; set `locations.cache` to keep them elsewhere. An id that another
location already has is prefixed with the folder's path; `entry.location` says where each model was found.

## Configuration and environment

`$TENSTAG_CONFIG`, else `$XDG_CONFIG_HOME/tenstag/config.toml`, else `~/.config/tenstag/config.toml`, with the
keys `defaults`, `[[location]]` (`path`, `kind` = directory, lmstudio, ollama, gpt4all, modelscope, jan, docker or ramalama,
`depth`, `min_size`, `added_by`, `added`) and `[names]`. (`class`, in 0.3.0 to 0.6.1, is no longer a key: a file with it is an error.)

`added_by` (an owner name) and `added` (a TOML date, or a date-time with its offset) say which program added a location
and when; `Location::added_by` and `Location::added` give them (`added` as `YYYY-MM-DD`, or `YYYY-MM-DDTHH:MM:SSZ` in
UTC). Versions before 0.10.0 take them as unknown keys, an error: every program on the machine should be current.

**Changing the file.** `add_to_config`, `remove_from_config` and `set_defaults` change only the lines they must; the
person's comments, order and `[names]` stay as written (tenstag specs/configuration.md CFG-010 to CFG-013). A change is
made under a lock (`config.toml.lock` beside the file), checked by reading the new text back as TOML, and written by a
rename. A file that cannot be read, or that says something in a form these rules cannot change safely (an array of
inline tables, say), is refused with `Error::Config` and left alone. An added path must be absolute or start with
`~/`. The file and its folder are created when needed; `TENSTAG_CONFIG` naming a missing file is an error. Whether to
ask the person before removing a location another program added (`added_by`) is your program's choice. A
`Locations` is not changed by these calls: `Locations::load()` again reads the new file.

A folder of your own (`kind = "directory"`, TENSTAG_PATH) is walked: a model is found when its path below the folder
has at most `depth` parts (`m.gguf` is 1, `org/m.gguf` and the model folder `org/m/` are 2), 6 unless `depth` says
otherwise. The walk reads each real folder once, so a link cannot make it loop, and it leaves out hidden folders,
`node_modules`, `__pycache__`, `site-packages` and Python virtual environments. LM Studio's and Ollama's stores are
read by their own layouts, so `depth` is an error there.

More language-model stores are found by themselves:
- GPT4All's model folder (`modelPath` in its settings, else its default folder): every `.gguf` file, at any depth to
  8 parts, is one model named by its file. Its downloads in progress and its remote-model files (which hold an API
  key) are never opened.
- The ModelScope cache (MODELSCOPE_CACHE, MS_CACHE_HOME, else `~/.cache/modelscope`), current and older layouts:
  each repository's GGUF files and model directories, named `modelscope:<owner>/<name>[:TAG][@revision]` so that a
  Hugging Face name never resolves to a ModelScope copy.
- Jan's data folder (JAN_DATA_FOLDER, else `data_folder` in its settings, else its default): one model per
  `model.yml`, named `jan:<model id>`; its quant tag comes from the id, as Jan saves downloads as `model.gguf`.
- Docker Model Runner's store (MODELS_PATH, else `~/.docker/models`): one model per tag, named `docker:<tag>`, from
  the GGUF layers of its manifest. With Docker Engine the store is inside a Docker volume that only root can read:
  name it as a location (`kind = "docker"`) to read it.
- RamaLama's store (RAMALAMA_STORE, else `store` in `ramalama.conf`, else `~/.local/share/ramalama`): one model per
  tag, named as RamaLama lists it, `<transport>://<organisation>/<name>:<tag>`.

A model these stores have not finished downloading is not listed.

An image tool's models folder (ComfyUI's `models/`, the Stable Diffusion web UI's or Forge's `models/`) is a folder
of your own: name it as a `[[location]]`, with a `depth` if its subfolders go deeper than 6. Each file is listed with
its path below the folder as its id (`loras/style/ink.safetensors`, `text_encoders/qwen_2.5_vl_7b.safetensors`);
which subfolder it is in, what that means to the tool, and whether your program can use the file, is for your
program to decide.

In a folder of your own, a file is listed as a model when its name says so: `.gguf` at any size, and `.safetensors`, `.sft`
and `.ckpt` from `min_size` bytes up (1 MiB unless `min_size` on the `[[location]]` says otherwise; 0 lists every
size). `.pt`, `.pth` and `.bin` files are not listed loose there: they are as often training state or caches.
A folder holding `config.json` and weights, `model_index.json` (a diffusers pipeline) or `adapter_config.json` (an
adapter) is one model, and nothing inside it is listed separately. A file given by its path is taken whatever its
name or size.

The locations are read in this order: the configuration's `[[location]]`s, TENSTAG_PATH, then the defaults (the
Hugging Face cache first). The same file reached twice is listed once, under its first id.

The cache folder, which holds the fingerprints and the tags (in `tags/`), is TENSTAG_CACHE_DIR when it is an absolute
path, else `$XDG_CACHE_HOME/tenstag` or `~/.cache/tenstag` (`~/Library/Caches/tenstag` on macOS,
`%LOCALAPPDATA%\tenstag` on Windows).

Variables: TENSTAG_CONFIG, TENSTAG_PATH, TENSTAG_CACHE_DIR, HF_HUB_CACHE, HUGGINGFACE_HUB_CACHE,
HF_HOME, XDG_CACHE_HOME, XDG_DATA_HOME, XDG_CONFIG_HOME, LLAMA_CACHE, OLLAMA_MODELS, and the other stores' own
variables above.

## Specifications

Everything follows the tenstag specification (tenstag/tenstag) at the commit in `spec-pin.toml`: the store layouts
(its specs/stores/), the configuration, the built-in locations, the listings, the names, the fingerprint, tags, tag
files and fingerprint cache. The code cites each rule it follows (`SPEC tenstag specs/<document>.md <rule>`);
`make spec` checks that every cited rule exists at that commit, and the tests run the specification's test
vectors.

## Known limits

- Tested on Linux only. Windows and macOS builds are compiled and linted on every change, but not run: their paths
  (cache folders, `\\?\` paths, symbolic links, file replacement, locks) are written from the
  specifications.
- On Windows a moved model is fingerprinted again (no file identity is read there yet); this costs time only.
- Ollama and LM Studio are tested against stores built to their documents, not against the programs.
- Paths that differ only in letter case, on a case-insensitive file system, are taken as different files in the fingerprint cache: such a file may be read twice.
- The interface is young (0.x): expect changes between minor versions, each listed in CHANGELOG.md.

## Requests and problems

Open an issue on the tenstag-rust repository, one per request or problem, saying what you expected, what happened, and
how to see it. Label it `request` for a change you would like, or `problem` for something that does not work as this
document says.
