# Commenting standard

This is tenstag's commenting standard (tensfiler's until 0.6.0). It began as the standard of the tens* projects it came from (tensmem,
tenslib, tensrun); it is now tenstag's own, and changes here need not follow theirs.

The people who maintain this code are humans who may never have read llama.cpp, transformers, or
the CUDA programming guide. Someone who knows Rust should be able to understand the code from its
comments: what each piece is for, why it is shaped the way it is, and which outside documents it
follows. The compiler checks some of these rules (see "Lints" below). The rest is up to review.

## 1. Every file starts with a module header

Use a `//!` block at the top of each file:

```rust
//! # GGUF tensor-info section
//!
//! Reads the list of tensor descriptions that follows the key/value metadata in a GGUF file.
//! Each entry gives a tensor's name, its shape (in ggml's innermost-first order), its ggml type
//! code, and its offset inside the data section.
//!
//! ```text
//!   header -> kv section -> [ tensor infos ] -> padding -> data section
//!                            ^^^^^^^^^^^^^^ this module
//! ```
//!
//! Spec: tenstag specs/fingerprint.md TFP-001 to TFP-007.
//!
//! This module does not read tensor *data*; see `data.rs`. It does not decide whether a type can be
//! computed with; it only checks that the type code is known (GGUF-011).
```

The header says four things:

1. What the module is for, in plain English.
2. Where it sits in the data flow. Use a small ASCII diagram for the larger modules.
3. Which spec sections or upstream documents it implements.
4. What it deliberately does **not** do, so readers don't go looking for it here.

## 2. Every public item has a doc comment

Use `///` doc comments that cover:

- **Purpose**: one sentence first; `cargo doc` uses it as the summary.
- **Units and invariants**: bytes or elements; row-major or ggml `ne` order; whether an offset is from
  the start of the file or of the data section; alignment guarantees.
- **`# Errors`** for every function that returns `Result`: which errors and when.
- **`# Panics`** if the function can panic, and why that can only happen through a programming error.
- **`# Safety`** for every `unsafe fn`: what the caller must guarantee.
- **`# Examples`** where a short example makes the use clearer than prose.

Private items get comments when their purpose is not obvious from their name and the module header.

## 3. Spec citations use one greppable form

When code follows a rule from the tenstag specification, say so on the line before it:

```rust
// SPEC tenstag specs/stores/ollama.md OLM-004 (IMPL): every blob is named sha256-<64 hex>.
```

The form is `SPEC tenstag specs/<document> <rule id> (<level>): <rule in your own words>`.
Then `grep -rn "specs/stores/ollama.md"` finds every place that depends on Ollama's layout. `make spec` checks
that every cited rule exists.

If a store document marks something below `documented` (`inferred` or `uncertain`), or it is a
suggestion (`SUG-nnn`, specs/stores/suggestions.md), say so too:

```rust
// CERTAINTY inferred (specs/stores/gpt4all.md G4A-002): Qt's choice of folder is from its docs, not its source.
// SUGGESTION SUG-041: which layers count as the model is a suggestion, not Ollama's own rule.
```

When the code follows an upstream source that no store document covers yet, cite that source
pinned to a commit, the way the store documents do:

```rust
// UPSTREAM llama.cpp 6050b467c07a src/llama-vocab.cpp: ...
```

## 4. Our own decisions are marked

When we choose between reasonable options, and no spec makes the choice, write it down:

```rust
// DECISION: the CPU allocator aligns to 64 bytes. That is one cache line on x86-64 and on most
// ARM cores, and it is enough for AVX-512 loads. We rejected page alignment (4096) for small
// buffers because it wastes memory on the many tiny allocations a KV cache makes.
```

Give the reason and the alternative we rejected. A later maintainer can then revisit the decision
knowing what was weighed.

## 5. Maths before optimised code

A kernel's comment states the maths first:

```rust
/// RMS normalisation of one row:
///
/// ```text
///   y_i = x_i / sqrt( (1/n) * sum_j x_j^2 + eps ) * w_i
/// ```
```

Each kernel has a plain, scalar reference version that follows the formula line by line. Every
optimised version (SIMD, CUDA) is tested against that reference. The reference is the documentation:
read it to see what the fast version is supposed to compute.

## 6. Every `unsafe` block says why it is sound

```rust
// SAFETY: `ptr` came from `alloc` with this same `layout` (see `CpuBuffer::new`), and `self`
// is being dropped, so nothing else can use the pointer afterwards.
unsafe { std::alloc::dealloc(ptr, layout) };
```

## 7. Tone

- Write in full sentences, for a reader who is new to the subject.
- Explain *why*. Don't repeat *what* the next line of code already says.
- Define a term the first time a file uses it (for example, "a *block* is the smallest unit a ggml
  quantised type can be cut into").
- Keep comments true. When you change the code, change the comment in the same commit.

## 8. Lints

Each crate's `Cargo.toml` sets these, and `make check` enforces them:

| lint | level | why |
|---|---|---|
| `missing_docs` | deny | every public item is documented |
| `unsafe_op_in_unsafe_fn` | deny | each unsafe operation gets its own `unsafe` block and `SAFETY` comment |
| `clippy::undocumented_unsafe_blocks` | deny | rule 6 |
| `clippy::missing_safety_doc` | deny | rule 2, `# Safety` |
| `clippy::missing_errors_doc` | warn | rule 2, `# Errors` |
| `clippy::missing_panics_doc` | warn | rule 2, `# Panics` |

`make check` turns warnings into errors, so in practice all of them must be clean before a commit.
