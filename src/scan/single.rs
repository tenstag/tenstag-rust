//! # Single model files: safetensors files and PyTorch checkpoints
//!
//! Image tools and people keep many models as one file: a whole Stable Diffusion checkpoint, a LoRA, a VAE, a text
//! encoder. This module says, from a file's name alone, whether it is such a file and which format, and handles sets
//! of safetensors files split as Hugging Face splits them:
//!
//! ```text
//!   sd_xl_base_1.0.safetensors                 Format::Safetensors (also .sft, ComfyUI's short name)
//!   v1-5-pruned.ckpt                           Format::Checkpoint (a Python pickle: listed, never opened)
//!   model-00001-of-00003.safetensors           the first part of a split set: one entry for the three files
//!   model-00002-of-00003.safetensors           (left out: a later part)
//! ```
//!
//! Nothing here opens a file; only names and sizes from the file system are used. Which files of a folder of your
//! own are listed (size, which endings) is decided by the walk in `mod.rs`.

use std::path::{Path, PathBuf};

use crate::scan::Format;

/// The format a file named `name` has, from its ending; `None` when the name does not say weights.
///
/// DECISION: `.safetensors`, `.sft` and `.ckpt` are always taken. `.pt`, `.pth`, `.pt2` and `.bin` only when `any`
/// is set (a path given directly, or a store that files models by use): elsewhere those endings are as often
/// training state, tokenizers or caches as models, and a filer cannot tell without opening them.
pub(crate) fn format_of(name: &str, any: bool) -> Option<Format> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".safetensors") || lower.ends_with(".sft") {
        Some(Format::Safetensors)
    } else if lower.ends_with(".ckpt") || (any && [".pt", ".pth", ".pt2", ".bin"].iter().any(|e| lower.ends_with(e))) {
        Some(Format::Checkpoint)
    } else {
        None
    }
}

/// A split set's name: `<base>-<index>-of-<count>.safetensors`, five digits each, as Hugging Face splits weights.
struct Split {
    base: String,
    index: u32,
    count: u32,
}

impl Split {
    fn parse(path: &str) -> Option<Split> {
        let stem = path.strip_suffix(".safetensors")?;
        let (rest, count) = stem.rsplit_once("-of-")?;
        let (base, index) = rest.rsplit_once('-')?;
        let digits = |t: &str| t.len() == 5 && t.bytes().all(|b| b.is_ascii_digit());
        if !digits(index) || !digits(count) {
            return None;
        }
        let (index, count) = (index.parse().ok()?, count.parse().ok()?);
        (1..=count).contains(&index).then(|| Split { base: base.to_string(), index, count })
    }

    fn part(&self, i: u32) -> PathBuf {
        PathBuf::from(format!("{}-{i:05}-of-{:05}.safetensors", self.base, self.count))
    }
}

/// Whether the file named `name` is listed for its set: a file not split, or the first part of a split set.
pub(crate) fn first_part(name: &str) -> bool {
    Split::parse(name).is_none_or(|s| s.index == 1)
}

/// Every file of the model whose file is `path`: the parts of its split set in order, or the file alone.
pub(crate) fn parts(path: &Path) -> Vec<PathBuf> {
    match Split::parse(&path.to_string_lossy()) {
        Some(split) => (1..=split.count).map(|i| split.part(i)).collect(),
        None => vec![path.to_path_buf()],
    }
}

/// The bytes of the model whose file is `path`: every part of a split set (0 when one is missing), or the file's.
pub(crate) fn bytes(path: &Path) -> u64 {
    parts(path).iter().map(|p| std::fs::metadata(p).map(|m| m.len()).ok()).sum::<Option<u64>>().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_say_the_format_and_split_sets_are_one_model() {
        assert_eq!(format_of("x.safetensors", false), Some(Format::Safetensors));
        assert_eq!(format_of("x.SFT", false), Some(Format::Safetensors));
        assert_eq!(format_of("x.ckpt", false), Some(Format::Checkpoint));
        assert_eq!(format_of("x.pt", false), None, "too common outside models in a folder of your own");
        assert_eq!(format_of("x.pt", true), Some(Format::Checkpoint));
        assert_eq!(format_of("x.json", true), None);

        assert!(first_part("m-00001-of-00003.safetensors") && !first_part("m-00002-of-00003.safetensors"));
        assert!(first_part("m-1-of-3.safetensors"), "not Hugging Face's five digits: a file of its own");
        assert_eq!(
            parts(Path::new("d/m-00001-of-00002.safetensors")),
            [PathBuf::from("d/m-00001-of-00002.safetensors"), PathBuf::from("d/m-00002-of-00002.safetensors")]
        );
    }
}
