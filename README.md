# tenstag

tenstag is a small Rust library for programs that run AI models. It finds the models already on your machine,
lets people refer to them by the names they already use, and gives your program somewhere to remember what it
worked out about each one.

I wrote it because every tool I used kept its models somewhere different: the Hugging Face cache, llama.cpp's
cache, LM Studio, Ollama, and a few folders of my own. Every program that wanted to run a model had to look in
all of those places itself, and then read every file again at each start to work out what it was. tenstag does the
looking once, for any program that wants it.

What it does:

- **Lists every model** wherever it was downloaded, without moving or copying anything.
- **Understands the names people type:** a path, a Hugging Face repository with an optional quant and revision
  (`org/name:Q4_K_M@main`), a nickname from the config file, or just a few words (`qwen3 0.6 q8`). For a
  repository name it picks the same file llama.cpp would.
- **Remembers what your program worked out** as tags, like `arch=qwen3` or `mem-4k=5.1GiB`. You read a model
  once, tag it, and next time the answer is already there. Other programs can read your tags too, in Rust, Python
  or anything else that follows the format.
- **Keeps tags with the model, not the path.** A model is recognised by a fingerprint of its contents (at most
  about 33 MiB is read, however big the file), so moving, renaming or copying it doesn't lose its tags.
- **Doesn't touch your models or the network.** It downloads nothing, changes no model, and only writes to its own
  cache folder (and the config file, if your program asks it to).

If you're new to it, the [introduction](https://github.com/tenstag/tenstag/blob/main/INTRODUCTION.md) in the specification
repository walks through a couple of example programs and how one config file can serve all of them.

It's a spare-time project, and it's still 0.x, so expect the odd rough edge and the occasional breaking change
(always listed in [CHANGELOG.md](CHANGELOG.md)).

## Adding it

For now, add it by git tag:

```toml
[dependencies]
tenstag = { git = "https://github.com/tenstag/tenstag-rust", tag = "v0.10.1" }
```

It needs Rust 1.99 or newer, and no system libraries.

## Opening a model by name

```rust,no_run
let mut library = tenstag::Library::open()?;
let entry = library.resolve("Qwen/Qwen3-0.6B-GGUF:Q8_0")?;    // a path, a nickname, an id, a repository, a search
println!("{}", entry.path.display());                         // a GGUF file, or a model directory
# Ok::<(), tenstag::Error>(())
```

A repository name finds whatever the Hugging Face cache holds for it, picked the way llama.cpp and huggingface_hub
pick. If the model isn't on the machine you get `Error::NotFound`; downloading it is up to your program (Hugging
Face's `hf-hub` crate writes to the same cache). If a name could mean more than one model you get
`Error::Ambiguous`, with the candidates so you can show them.

## Tagging models

Your program opens the library as the *owner* of its tags, with a version number. It can only write its own tags,
but it can read everyone's. The usual pattern is: look for a tag, and if it's missing, work it out from the model
and tag it. For example, a runner starting up:

```rust,no_run
# fn read_architecture(_: &std::path::Path) -> String { String::new() }
let mut library = tenstag::Library::open_as("com.example.myapp", 1)?; // owner and version
library.only(|e| e.format == tenstag::Format::Gguf);
for entry in library.entries().to_vec() {
    if !library.tags(&entry)?.has("arch") {
        let arch = read_architecture(&entry.path);          // your code: the slow part, once per model
        library.tag(&entry, &format!("arch={arch}"))?;
        library.tag(&entry, "needs=t5xxl,clip-l")?;         // a value may hold several items
    }
}

let qwen3 = library.tagged("arch=qwen3")?;                 // exactly that value; "arch" for any value
let t5 = library.tagged_with("needs", "t5xxl")?;           // "t5xxl" is one of the value's items
let tags = library.tags(&qwen3[0])?;
tags.of("com.example.imagenode").value("encoder");                     // another program's tags, read-only
let unets = library.tagged_by("com.example.imagenode", "role=unet")?;
# Ok::<(), tenstag::Error>(())
```

Each key has one value, so setting `arch=llama` replaces `arch=qwen3`. `tags.value(key)` gives you your value
exactly as you wrote it, and `tags.value_set(key)` splits it on commas. If you change how you work something out,
bump the version: your older tags then count as missing, and each model gets worked out again once.
`library.untag_all(&entry)` removes all of your tags from a model, and never touches anyone else's.

**Tags are a cache.** They live in the cache folder, and people can delete that whenever they like; your program
just works its tags out again next time. So only put things in tags that you can work out again from the model
itself. Something only a person can tell you (a favourite, a rating) belongs in your own settings. You can copy it
into a tag if you want other programs to see it, but write it again when it's missing.

Tags follow the model's contents, so the same model in two stores, or moved to another disk, has the same tags.
If a model's file changes, it counts as a new model with no tags; `library.previous(&entry)` gives you the tags it
used to have. `library.verify(&entry, ...)` computes a full SHA-256 when you need every byte checked. Search
(`search`, and the last step of `resolve`) looks at tags as well as names.

Pick an owner name nobody else will pick, or two programs will overwrite each other's tags. A **reverse domain
name** you control plus the program works (`com.example.myapp` if you own `example.com`), and so does your
program's **GitHub repository** in lower case (`example/myapp`). Owner names use lowercase letters, digits, `-`,
`_`, `.` and `/`; keys are any text without spaces.

Owners are a convention, not a lock. Nothing checks the name, and the tag files are plain JSON that any program
can change, so treat other programs' tags as untrusted input and work out again anything that looks wrong. The
format is written down in the tenstag specification (tenstag/tenstag), so programs in any language can use it.

## Listing and searching

```rust,no_run
use tenstag::Library;

let mut library = Library::open()?;
for entry in library.entries() {
    println!("{:<50} {:>12} bytes  {}", entry.id, entry.bytes, entry.location.display());
}
for entry in library.search("qwen3 q8") {                  // every word must match the id, the file or the tags
    println!("{}", entry.path.display());
}
# Ok::<(), tenstag::Error>(())
```

Each `Entry` tells you where the model is (`path`), how it's stored (`format`: a GGUF file, a model directory, a
safetensors or checkpoint file, a diffusers pipeline, an adapter), its `quant`, its size, which store it came from,
and, for the Hugging Face cache, its repository and commit. `entry.files()` lists every file that belongs to it.

tenstag doesn't decide what a model is *for*. A GGUF file could be a chat model, an image tool's quantised UNet, or
a text encoder (some of which are full language models). Your program decides with its own rule, and from then on
listings, searches, tag queries and names only show what's left:

```rust,no_run
use tenstag::{Format, Library};

let mut library = Library::open()?;
library.only(|e| matches!(e.format, Format::Gguf | Format::HfDirectory));
let found = library.resolve("qwen3 0.6")?;       // not a loose Qwen encoder .safetensors in an image folder
# Ok::<(), tenstag::Error>(())
```

Whatever your program learns by opening a model (it's a chat model, a text encoder, this architecture) it can
keep as a tag, such as `kind=chat`, for itself and for other programs.

## Where it looks

| place | found through |
|---|---|
| the Hugging Face cache | HF_HUB_CACHE, HF_HOME, else `~/.cache/huggingface/hub` |
| llama.cpp's cache | LLAMA_CACHE, else `~/.cache/llama.cpp` |
| LM Studio | its home pointer, else `~/.cache/lm-studio` or `~/.lmstudio` |
| Ollama | OLLAMA_MODELS, else `~/.ollama/models` (and the Linux service's store) |
| GPT4All | its settings' `modelPath`, else `~/.local/share/nomic.ai/GPT4All` |
| the ModelScope cache | MODELSCOPE_CACHE, MS_CACHE_HOME, else `~/.cache/modelscope` (models named `modelscope:org/name`) |
| Jan | its settings' `data_folder`, else its data folder (models named `jan:<id>`) |
| Docker Model Runner | MODELS_PATH, else `~/.docker/models` (models named `docker:<tag>`) |
| RamaLama | RAMALAMA_STORE or `ramalama.conf`, else `~/.local/share/ramalama` (`huggingface://org/name:tag`) |
| ComfyUI, Automatic1111, Forge | where you name their models folder, as a `[[location]]` |
| your folders | TENSTAG_PATH (`:`-separated), or `[[location]]` in the configuration |

All of these are only ever read. Fingerprints and tags are cached in `~/.cache/tenstag` (set TENSTAG_CACHE_DIR to
move it). macOS and Windows use their own folders; USAGE.md has the full list.

An image tool's models folder is just a folder: tenstag lists each `.safetensors`, `.ckpt` or `.gguf` file and each
diffusers pipeline with its path below the folder, and leaves it to your program to decide what `loras/` or `vae/`
means.

## Configuration

You don't need one, but if you want it, it's `~/.config/tenstag/config.toml` (or wherever `$TENSTAG_CONFIG`
points):

```toml
defaults = true                    # false: look only in the locations listed here

[[location]]
path = "/mnt/nas/models"           # a folder of your own models
depth = 10                         # how deep to look (6 by default)
min_size = 0                       # list loose .safetensors/.ckpt files of any size (1 MiB by default)
[[location]]
path = "/srv/ollama/models"
kind = "ollama"                    # another program's store: "ollama", "lmstudio", "gpt4all",
                                   # "modelscope", "jan", "docker" or "ramalama"
[[location]]
path = "~/ComfyUI/models"          # an image tool's models: listed file by file, as they are,
depth = 8                          # with their paths below it (unet/..., text_encoders/...)

[names]
small = "Qwen/Qwen3-0.6B-GGUF:Q8_0"   # a nickname, resolved like any name
```

## Your program's own folders

If your program keeps models in its own folders, it can hand them to tenstag directly, with nothing in anyone's
config file. `Locations::bare()` ignores the config file and the usual places; `Locations::load()` starts from
the user's setup and adds to it. Either way, tags go in the same cache folder as every other program's, unless you
change `locations.cache`.

```rust,no_run
use tenstag::{Library, Location, Locations};

let mut locations = Locations::bare();                       // or Locations::load()? to keep the user's places too
locations.add(Location::folder("/srv/myapp/models").depth(3)); // a model is at most 3 path parts below it
locations.add(Location::folder("/srv/myapp/images").what("my app's image models"));
let mut library = Library::new(locations);
# Ok::<(), tenstag::Error>(())
```

## More

- [USAGE.md](USAGE.md): every call and what it promises, the name rules, the environment variables, and the
  known limits.
- [CHANGELOG.md](CHANGELOG.md): what changed in each version.
- [COMMENTING.md](COMMENTING.md): how the code is commented, if you want to read or change it. `make check` runs
  the formatting, lint, documentation and test checks.

How tenstag reads each store, the fingerprint and the tag files all follow the [tenstag specification](https://github.com/tenstag/tenstag)
(tenstag/tenstag), including a written description of each store's layout. `spec-pin.toml` says which version, and
the code notes which rule it's following as it goes.

(Up to 0.5.0 it was called tensfiler, and downloaded models too.)

## Licence

Copyright 2026 Nick Reynolds (goldnode).

tenstag is licensed under either of

- the Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), or
- the MIT licence ([LICENSE-MIT](LICENSE-MIT)),

at your option.

Unless you say otherwise, any contribution you submit for inclusion in tenstag is dual-licensed as above, with no
extra terms or conditions.
