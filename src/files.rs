//! # GGUF file names: which files are models, how split files are named, and which file a quant tag picks
//!
//! A Hugging Face repository of GGUF files usually holds one model in several quantizations
//! (`Qwen3-0.6B-Q4_K_M.gguf`, `Qwen3-0.6B-Q8_0.gguf`, ...), sometimes split into parts
//! (`...-00001-of-00003.gguf`), with side files that are not models (a vision projector `mmproj-...`, an
//! importance matrix). A *quant tag* such as `Q4_K_M` names one of the quantizations: llama.cpp's
//! `-hf <org>/<repo>:<tag>` picks a file by it, and so does tenstag, so that both pick the same file.
//!
//! Spec: tenstag specs/stores/hf-hub-cache.md HFC-010 (IMPL), llama.cpp's `find_best_model`.
//!
//! This module only looks at names; it reads no file.

/// Words that mark a GGUF file as a side file of a model rather than a model: a vision projector and the
/// drafting heads llama.cpp downloads beside a model.
///
/// SPEC tenstag specs/stores/hf-hub-cache.md HFC-010 (IMPL): the sidecar keywords.
const SIDECAR_WORDS: [&str; 5] = ["mmproj", "mtp-", "dflash-", "eagle3-", "dspark-"];

/// Whether `path` (a file name, or a path inside a repository) names a GGUF model: it ends in `.gguf`, and its
/// file name holds neither "imatrix" nor a side file's word (`mmproj`, `mtp-`, ...).
///
/// SPEC tenstag specs/stores/hf-hub-cache.md HFC-010 (IMPL): a model file.
pub fn is_gguf_model(path: &str) -> bool {
    if !path.ends_with(".gguf") {
        return false;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    !name.contains("imatrix") && !SIDECAR_WORDS.iter().any(|word| name.contains(word))
}

/// What a GGUF file's name says about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GgufName {
    /// The path without `.gguf` and without a split suffix: the same for every part of a split model.
    pub prefix: String,
    /// The quant tag at the end of `prefix`, upper case (`Q4_K_M`), or empty when the name has none.
    pub tag: String,
    /// This part's number, from 1; 1 for a file that is not split.
    pub index: u32,
    /// How many parts the model has; 1 for a file that is not split.
    pub count: u32,
}

impl GgufName {
    /// Reads a GGUF path's name, or `None` when it does not end in `.gguf`.
    ///
    /// The split suffix is `-NNNNN-of-MMMMM` (five digits each, any case of "of"); the tag is the last run of
    /// letters, digits and underscores after a `-` or `.`.
    ///
    /// SPEC tenstag specs/stores/hf-hub-cache.md HFC-010 (IMPL) gives the split suffix. The tag read from a name (for
    /// listings, not for choosing) is UPSTREAM llama.cpp 7bcbc04fb common/download.cpp 537-566 (`get_gguf_split_info`:
    /// `[-.]([A-Z0-9_]+)$`, case-insensitive), not in the spec: request 20261122-01.
    pub fn parse(path: &str) -> Option<GgufName> {
        let mut prefix = path.strip_suffix(".gguf")?;
        let (mut index, mut count) = (1, 1);
        if let Some((rest, i, n)) = split_suffix(prefix) {
            (prefix, index, count) = (rest, i, n);
        }
        let end = prefix.len() - prefix.bytes().rev().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_').count();
        let tag = match prefix[..end].bytes().last() {
            Some(b'-' | b'.') if end < prefix.len() => prefix[end..].to_ascii_uppercase(),
            _ => String::new(),
        };
        Some(GgufName { prefix: prefix.to_string(), tag, index, count })
    }

    /// The path of part `index` (from 1) of this model: the prefix again for a file that is not split.
    pub fn part(&self, index: u32) -> String {
        if self.count == 1 {
            format!("{}.gguf", self.prefix)
        } else {
            format!("{}-{index:05}-of-{:05}.gguf", self.prefix, self.count)
        }
    }
}

/// `prefix-NNNNN-of-MMMMM` split into (prefix, N, M), or `None`.
fn split_suffix(name: &str) -> Option<(&str, u32, u32)> {
    let (rest, count) = name.rsplit_once('-')?;
    let (rest, of) = rest.rsplit_once('-')?;
    let (rest, index) = rest.rsplit_once('-')?;
    let five = |s: &str| s.len() == 5 && s.bytes().all(|b| b.is_ascii_digit());
    if rest.is_empty() || !of.eq_ignore_ascii_case("of") || !five(index) || !five(count) {
        return None;
    }
    Some((rest, index.parse().ok()?, count.parse().ok()?))
}

/// Whether `path` carries `tag` as a whole token: preceded by the start, `-`, `.` or `/`, and followed by `.` or
/// `-`, ignoring case. So `F16` does not match a `BF16` file.
///
/// SPEC tenstag specs/stores/hf-hub-cache.md HFC-010 (IMPL): the path matches `(^|[-./])<tag>[.-]`, ignoring case.
pub fn has_tag(path: &str, tag: &str) -> bool {
    if tag.is_empty() {
        return false;
    }
    let (path, tag) = (path.to_ascii_lowercase(), tag.to_ascii_lowercase());
    path.match_indices(&tag).any(|(at, _)| {
        let before = at.checked_sub(1).map(|i| path.as_bytes()[i]);
        let after = path.as_bytes().get(at + tag.len()).copied();
        matches!(before, None | Some(b'-' | b'.' | b'/')) && matches!(after, Some(b'.' | b'-'))
    })
}

/// The tags tried when none is given, in order.
///
/// SPEC tenstag specs/stores/hf-hub-cache.md HFC-010 (IMPL): Q4_K_M, then Q8_0.
pub const DEFAULT_TAGS: [&str; 2] = ["Q4_K_M", "Q8_0"];

/// The model file `tag` picks among `paths` (a repository's file list), as llama.cpp's `-hf` picks it: the first
/// GGUF model file, in the list's order, that carries the tag ([`has_tag`]) and is the first part of its model;
/// with no tag, [`DEFAULT_TAGS`] in turn, then the first model file. `None` when nothing fits.
///
/// SPEC tenstag specs/stores/hf-hub-cache.md HFC-010 (IMPL), over the files in the order the Hub's tree listing gives
/// them.
pub fn best_model<'a>(paths: impl IntoIterator<Item = &'a str> + Clone, tag: Option<&str>) -> Option<&'a str> {
    let first_part = |path: &str| is_gguf_model(path) && GgufName::parse(path).is_some_and(|n| n.index == 1);
    let tags: Vec<&str> = match tag {
        Some(tag) => vec![tag],
        None => DEFAULT_TAGS.to_vec(),
    };
    for tag in tags {
        if let Some(path) = paths.clone().into_iter().find(|p| first_part(p) && has_tag(p, tag)) {
            return Some(path);
        }
    }
    if tag.is_none() { paths.into_iter().find(|p| first_part(p)) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_read_as_llama_cpp_reads_them() {
        let n = GgufName::parse("Qwen3-0.6B-Q8_0.gguf").unwrap();
        assert_eq!((n.prefix.as_str(), n.tag.as_str(), n.index, n.count), ("Qwen3-0.6B-Q8_0", "Q8_0", 1, 1));
        let n = GgufName::parse("big/Model-q4_k_m-00002-of-00003.gguf").unwrap();
        assert_eq!((n.prefix.as_str(), n.tag.as_str(), n.index, n.count), ("big/Model-q4_k_m", "Q4_K_M", 2, 3));
        assert_eq!(n.part(1), "big/Model-q4_k_m-00001-of-00003.gguf");
        assert_eq!(GgufName::parse("model.gguf").unwrap().tag, "");
        assert_eq!(GgufName::parse("plain").map(|n| n.tag), None);
    }

    #[test]
    fn side_files_are_not_models() {
        assert!(is_gguf_model("org/Qwen3-Q8_0.gguf"));
        assert!(!is_gguf_model("mmproj-Qwen3-F16.gguf"));
        assert!(!is_gguf_model("Qwen3.imatrix.gguf"));
        assert!(!is_gguf_model("README.md"));
    }

    #[test]
    fn a_tag_picks_the_file_llama_cpp_picks() {
        let files = [
            "README.md",
            "m-BF16.gguf",
            "m-F16.gguf",
            "mmproj-m-Q8_0.gguf",
            "m-Q8_0.gguf",
            "split/m-Q4_K_M-00002-of-00002.gguf",
            "split/m-Q4_K_M-00001-of-00002.gguf",
        ];
        let it = files.iter().copied();
        assert_eq!(best_model(it.clone(), Some("f16")), Some("m-F16.gguf"));
        assert_eq!(best_model(it.clone(), Some("q8_0")), Some("m-Q8_0.gguf"));
        assert_eq!(best_model(it.clone(), None), Some("split/m-Q4_K_M-00001-of-00002.gguf"));
        assert_eq!(best_model(it.clone(), Some("Q2_K")), None);
        assert_eq!(best_model(["a-IQ2_XS.gguf"].iter().copied(), None), Some("a-IQ2_XS.gguf"));
    }
}
