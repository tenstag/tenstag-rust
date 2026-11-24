# Changes

The interface is USAGE.md. While the version is 0.x, a change that could break a program takes a new minor version;
an addition or a fix takes a new patch version.

## 0.10.1 (2026-11-24)

Nothing a program uses changes.

- Licensed under MIT OR Apache-2.0, at your option (it was MIT): LICENSE is now LICENSE-MIT, with LICENSE-APACHE
  beside it.
- The store layouts are now cited from the tenstag specification's own copy of them (specs/stores/), so
  `make spec` needs only a tenstag specification checkout.
- The documentation is plainer, and points at the GitHub repositories (github.com/tenstag).

## 0.10.0 (2026-11-24)

A program can change the configuration file for the person (tenstag-rust#2, from tensrun): add a folder, remove one,
turn the defaults off; and a location records which program added it and when (tenstag specs/configuration.md CFG-009
to CFG-014).

- `Locations::add_to_config(&location, added_by)` -> `AddedToConfig::Added` or `AlreadyListed`,
  `Locations::remove_from_config(path)`, `Locations::set_defaults(on)`, `Locations::config_path()`. Only the lines a
  change needs are touched; comments, order and `[names]` stay. Locked, checked by reading back, written by a rename;
  a file that cannot be changed safely is refused.
- `Locations::default_locations()`: the built-in locations even when `defaults = false`.
- `[[location]]` keys `added_by` (an owner name) and `added` (a TOML date or offset date-time), read as
  `Location::added_by` and `Location::added`.
- **Breaking:** `Location` has two new public fields, so a struct literal needs them (`Location::new` and
  `Location::folder` are unchanged). Versions before 0.10.0 reject a configuration file with `added_by` or `added`.

## 0.9.2 (2026-11-23)

- Ollama: a model in another format than GGUF whose tensor blobs are not all present counts the bytes of those that
  are (it counted none). tenstag specs/listing.md LST-008, clarified when tenstag-python was written from the
  documents.
- `examples/tags.rs`: prints a model file's identity and every owner's tags, to check that implementations in other
  languages read and write the same files.

- Docs: owners are a courtesy between well-behaved programs, not a protection (tenstag specs/tags.md TAG-012).
  Nothing checks an owner name and the tag files are plain files, so treat the tags you read as unchecked input.

## 0.9.1 (2026-11-23)

- Owner names may hold `/` (one between two other characters) and be up to 128 bytes, so that a program can use its
  GitHub repository, `user/repository` in lower case, as its owner name. The docs ask for an owner name no other
  program will choose: a reverse domain name you control (`com.example.myapp`) or that GitHub repository (tenstag
  specs/tags.md TAG-011).

## 0.9.0 (2026-11-23)

Every tag has an owner: the program that wrote it, under a name and a version it gives when it opens the library. A
program writes only its own tags and reads anyone's, so keys need no prefix, and a new version of a program sees its
older tags as missing and works them out again (tenstag specs/tags.md TAG-009, TAG-010; tag files are
`tenstag-tags/2`).

**Changes a program must make** (the version is 0.x, so this is a new minor version):
- Open the library as the owner of your tags: `Library::open_as("myapp", 1)`, or `library.set_owner("myapp", 1)`.
  Without an owner, a library reads tags, and `tag`, `untag`, `tagged` and `tagged_with` are `Error::Tag`.
- Drop the prefix from your keys: `myapp/arch=qwen3` is now `arch=qwen3`, owned by `myapp`. Drop a version tag of your
  own (`myapp/version=2`): give the version to `open_as` instead.
- `Tags` is now every owner's tags: `value`, `has` and `value_set` read your own (`tags.mine()`); `tags.of(owner)`
  reads another's; `OwnerTags` is one owner's, with its `version()`. `Tags::new` is no longer public.
- Tag files written by 0.6.0 to 0.8.0 (`tenstag-tags/1`, without owners) are read as no tags and replaced at the next
  write: your program works its tags out again once.

**New:** `Library::untag_all` (all your tags of a model, never another's), `tagged_by` (another owner's tags),
`tagged_where` (any test across owners), `Library::owner`, `is_owner`.

## 0.8.0 (2026-11-23)

Tags are a cache of what programs work out about models: they are kept in the cache folder with the fingerprints,
and a person may delete them at any time (tenstag specs/tags.md TAG-008, specs/tag-store.md TGS-001).

**Changes a program must make** (the version is 0.x, so this is a new minor version):
- `Locations::data` and `TENSTAG_DATA_DIR` are gone. Tags are in `tags/` in the cache folder, `Locations::cache`
  (TENSTAG_CACHE_DIR, else `~/.cache/tenstag`). Tags written by 0.6.0 to 0.7.1 in `~/.local/share/tenstag` are not
  read: work them out again, as after any clearing of the cache, and delete that folder.
- Keep in tags only what your program can work out again from a model; when a tag is missing, work it out and tag
  the model again. Something only a person can say, such as a favourite, belongs in your program's own settings.

## 0.7.1 (2026-11-23)

- RamaLama: when a tag has both a JSON ref file and an older text one, the JSON file is read (the text one was).
- A Hugging Face repository name with no quant, `org/name`, resolves to a cached diffusers pipeline or adapter as it
  did to a model folder.
- Everything programs see through the configuration, the locations, the listings and the names is now specified in
  the tenstag specification (configuration.md, locations.md, listing.md, names.md), and this crate's tests run its
  hand-written cases.

## 0.7.0 (2026-11-23)

tenstag no longer says what kind of model a file is: it gives facts (format, path, id, location), and the program
decides, as it may then record with a tag. The guess was wrong both ways: an image tool's GGUF UNets are not
language models, and its text encoders may be.

**Changes a program must make** (the version is 0.x, so this is a new minor version):
- `Class` and `Entry::class` are gone, and so are `Location::class` and the configuration's `class` key (a
  configuration file with it is an error that says so).
- `Library::only` takes the program's own rule, a function from `&Entry` to whether to keep it, instead of a list of
  classes: `library.only(|e| matches!(e.format, Format::Gguf | Format::HfDirectory))` for a language-model runner;
  `only(|_| true)` keeps every model again.

## 0.6.1 (2026-11-23)

- `Library::sha256(&entry)`: a one-file model's SHA-256 when it is known without reading it, from the name its
  store gives the file (a Hugging Face cache LFS blob, an Ollama blob, a Docker Model Runner `sha256` blob; never a
  RamaLama blob), or from an earlier `verify`. It is also recorded in the fingerprint cache and the tag file, with
  `sha256_from` saying `store` or `read` (tenstag specs/fingerprint-cache.md TFC-006).

## 0.6.0 (2026-11-23)

tensfiler is now **tenstag**: an offline library that finds models and tags them by their contents. It no longer
downloads anything, and its shared metadata is replaced by tags.

**Changes a program must make** (the version is 0.x, so this is a new minor version):
- The crate is `tenstag` (tag `v0.6.0`).
- Its variables are renamed: `TENSFILER_CONFIG` is `TENSTAG_CONFIG` and `TENSFILER_PATH` is `TENSTAG_PATH`. The
  configuration file is `~/.config/tenstag/config.toml`: move `~/.config/tensfiler/config.toml` there, without
  `download` and `offline`, which are no longer keys (a file with them is an error).
- Downloads are gone: `Hub`, `Plan`, `Progress`, `Library::locate`, `Located`, and `Error::Hub` and
  `Error::Checksum`. `TENSFILER_DOWNLOAD`, `Locations::download` and `Locations::offline` are gone too. A program
  that downloads uses its own downloader (Hugging Face's `hf-hub` crate writes the cache tenstag reads).
- `Library::resolve` returns the `Entry`; `Found` is gone. A Hugging Face repository name resolves to the model the
  cache holds (as `Library::cached` chooses), or `Error::NotFound`.
- `HubRef::tag` is `HubRef::quant`, and `HubRef::file` (one file of a repository, provisional in 0.5.0) is gone; a
  cached single file is found by its id.
- `Entry::tag` (the quant tag) is `Entry::quant`, so that *tag* means only the new tags.
- Shared metadata is gone: `Library::meta`, `set_meta`, `remove_meta`, `clean_meta`, `Stamp`, `valid_namespace`,
  `Error::Meta`, `Locations::meta` and `TENSFILER_META_DIR`. Use tags (`myprogram/key=value`). tensfiler-meta/1
  files are neither read nor deleted.
- `Locations::bare()` has no locations at all (it had the download cache); add the Hugging Face cache yourself if
  you want it.

**New:**
- Tags: `Library::tag`, `untag`, `tags`, `tagged`, `tagged_with`, `previous`; `Tags` with `has`, `value`,
  `value_set`, `words`, `iter`; `parse_tag`, `value_set`; `Error::Tag`. A key has one value; setting it again
  replaces it.
- Identity by content: `Library::identity`, `Identity`, `fingerprint_file`, and `Library::verify` for a full
  SHA-256. Fingerprints are cached, and follow a file moved on the same file system (Unix).
- `Locations::data` (the tag folder: TENSTAG_DATA_DIR, else `~/.local/share/tenstag`) and `Locations::cache`
  (TENSTAG_CACHE_DIR, else `~/.cache/tenstag`); `Library::clean_tags`.
- `search` matches tags too.
- Their formats, and the fingerprint `tfp1`, are specified in the tenstag specification (tenstag/tenstag), with
  test vectors this crate runs.

## 0.5.0 (2026-11-23)

A program can give its own folders without any configuration, and fetch single files or files it chooses.

**Changes a program must make** (the version is 0.x, so this is a new minor version):
- `HubRef` has a new field, `file`: a `HubRef { .. }` written out in full needs `file: None`.
- `Source` is `#[non_exhaustive]` and has `Source::Program`: a `match` on it needs a `_` arm.

**Added:**
- `Locations::bare()` and `bare_from(env)`: only the download cache, reading no configuration file and no default
  locations. `Locations::add(location)`; `Location::new(path, kind)` and `Location::folder(path)`, with `.depth`,
  `.min_size`, `.class` and `.what`. A program's folders need no line in the user's configuration; their records go
  in the usual metadata folder.
- Provisional: `org/name:path/file` names one file of a repository (`HubRef::file`), located, fetched and found
  offline alone; `Hub::listing` gives every file of a revision for the program to choose from before `Hub::fetch`,
  which now refuses a plan whose files are not repository paths.

## 0.4.0 (2026-11-23)

tensfiler no longer reads image tools' layouts: an image tool's models folder is named as a folder of your own, and
its files are listed as they are, leaving what each subfolder means to the program using them.

**Changes a program must make** (the version is 0.x, so this is a new minor version):
- `kind = "comfyui"` and `kind = "sdwebui"` are gone, with `Kind::ComfyUi`, `Kind::SdWebui` and `ROLE_DEPTH`, and
  ComfyUI Desktop's folder is no longer found by itself. Name the models folder as a `[[location]]` instead, with
  `class = "diffusion"` and, if it is deep, `depth = 8`.
- `Entry::role` is gone; an entry's id is its path below the folder, which holds the same word.
- `Class` keeps `Language`, `Diffusion`, `Adapter` and `Unknown`; `Encoder`, `Vae`, `Upscaler` and `Other` are gone.

**Added:**
- `class` on a `[[location]]` of kind `directory` (`Location::class`): the class of its GGUF files, model directories
  and loose files.

## 0.3.2 (2026-11-23)

- Jan's models (tensdataspecs stores/jan.yaml), Docker Model Runner's store (stores/docker-model-runner.yaml) and
  RamaLama's store (stores/ramalama.yaml), found by themselves; `kind = "jan"`, `"docker"` and `"ramalama"` for
  others. `Kind` gains `Jan`, `DockerModels` and `RamaLama`. Names: `jan:<id>`, `docker:<tag>`,
  `<transport>://<organisation>/<name>:<tag>`.

## 0.3.1 (2026-11-23)

- GPT4All's model folder (tensdataspecs stores/gpt4all.yaml) and the ModelScope cache (stores/modelscope-cache.yaml),
  found by themselves; `kind = "gpt4all"` and `kind = "modelscope"` for others. `Kind` gains `Gpt4All` and
  `ModelScope`. ModelScope's models are named `modelscope:<owner>/<name>[:TAG][@revision]`.

## 0.3.0 (2026-11-23)

Image and video models, as well as language models: found, named and classed by what their stores and layouts say,
never by opening them.

**Changes a program must make** (the version is 0.x, so this is a new minor version):
- `Format` and `Kind` are `#[non_exhaustive]` and have new variants: a `match` on either needs a `_` arm.
- A program that runs only language models calls `library.only(&[Class::Language])` after opening the library, so
  that listings, searches and names leave image models and loose files of unknown kind out.

**Added:**
- `Format::Safetensors` (a single file, or a split set by its first part), `Format::Checkpoint` (`.ckpt`, and `.pt`,
  `.pth`, `.bin` where a store files them; listed, never opened), `Format::Diffusers` (a pipeline folder,
  `model_index.json`) and `Format::Adapter` (a PEFT adapter folder, `adapter_config.json`).
- `Entry::class` (`Class`: `Language`, `Diffusion`, `Adapter`, `Encoder`, `Vae`, `Upscaler`, `Other`, `Unknown`) and
  `Entry::role` (an image tool's own folder word for a model's use). `Library::only`. Search also matches `role`.
- Image tools' stores, read by their role folders: `kind = "comfyui"` (ComfyUI; ComfyUI Desktop is found by itself
  from the folder it records) and `kind = "sdwebui"` (the Stable Diffusion web UI and Forge). Role folders are walked
  `ROLE_DEPTH` (8) deep.
- Folders of your own: loose `.safetensors`, `.sft` and `.ckpt` files from `min_size` bytes (1 MiB by default;
  `DEFAULT_MIN_SIZE`), split safetensors sets as one model, diffusers pipelines and PEFT adapters. `depth` on a
  `[[location]]` sets how deep a folder is walked (6 by default, `DEFAULT_DEPTH`; `Kind::default_depth`).
  `Location` gains `depth` and `min_size`.
- The Hugging Face cache: a snapshot holding `model_index.json` is one diffusers pipeline; a snapshot that is no model
  directory lists the single `.safetensors` and `.ckpt` files fetched into it (`org/name/file.safetensors`).
- Shared metadata for every new kind of model (tensdataspecs stores/tensfiler-meta.yaml TFM-002 and TFM-005: a
  pipeline's stamp covers its component folders).

**Changed:**
- A walk can no longer loop through a link, and leaves out `node_modules`, `__pycache__`, `site-packages` and Python
  virtual environments. A model folder may now be as deep as a model file.
- A folder holding both `config.json` and `adapter_config.json` is a model directory, as the metadata format says.
- Windows: a download's snapshot entry is a symbolic link where Windows allows one (Developer Mode), and a copy only
  where it does not, as huggingface_hub does; it was always a copy, so every downloaded file was stored twice. Not yet
  run on Windows.
- Windows and macOS builds are compiled and linted on every push (`make cross`); they are still not run.
- Follows tensdataspecs 3f5d9fb (spec-pin.toml).

**Not yet:** downloading a diffusers pipeline or a single file (`Hub` fetches GGUF files and model directories as
before); the extra folders of ComfyUI's `extra_model_paths.yaml`.

## 0.2.3 (2026-11-22)

- A metadata file of a later format version is now left as it is: `set_meta` and `remove_meta` write nothing for
  that model, where they used to replace the file (tensdataspecs TFM-006 and TFM-009, as refined in b1bb799).
- Follows tensdataspecs b1bb799 (spec-pin.toml).

## 0.2.2 (2026-11-22)

- `Library::clean_meta`: deletes the metadata files no program can use any more, those of models that changed or
  are gone (tensdataspecs TFM-010). It is a new call; nothing else changes.
- README.md is rewritten for the people who use the library, and its examples are now compiled with the tests.

## 0.2.1 (2026-11-22)

- `Source` (the type of `Location::source`) is exported.
- USAGE.md (the interface), the maintainers' notes, spec-pin.toml and
  `make spec` (every cited specification rule checked against the pinned commit).
- Its own CI: `make check` on runners labelled `proxmox`.
- Built with Rust 1.100.0 (rust-toolchain.toml); `rust-version` is unchanged.

## 0.2.0 (2026-11-22)

- Every store follows its tensdataspecs document (spec-pin.toml): hf-hub-cache, llama-cpp-cache, ollama, lm-studio,
  and the metadata format tensfiler-meta.
- A Hugging Face repository name is `Found::Hub`; `Library::locate` asks the Hub which file it means (the cache only
  offline), returning `Located::Here` or `Located::Fetch(Plan)`; `Hub::fetch` downloads a plan. (`Found::Missing` is
  gone.) A revision may be a branch, tag, or full or short commit.
- Shared metadata replaces the catalogue: `Library::meta`, `set_meta` (with `Stamp::of` taken before reading the
  model), `remove_meta`; one file per model in `tensfiler-meta/1`. (`Describe`, `ModelInfo` and
  `Library::save` are gone.)
- `Kind::LmStudio`; Ollama's manifest lists, split and safetensors models (`Format::Other`, `Entry::parts`,
  `Entry::note`); `Entry::files`.
- Modules are private: the interface is what `lib.rs` exports.

## 0.1.0 (2026-11-22)

- Locations, scanning, names, a catalogue through `Describe`, and downloads from the Hugging Face Hub.
