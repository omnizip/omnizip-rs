//! In-process synthetic corpora — no network required.
//!
//! Used for smoke-testing the bench itself and for CI runs where
//! downloading Silesia/Enwik8 is too slow. Each variant generates a
//! deterministic byte sequence (seeded RNG, no time source).

use crate::corpus::{Corpus, CorpusFile, CorpusError};

/// Names of the synthetic corpora known to [`all`].
pub const NAMES: &[&str] = &["zeros", "random", "text", "mixed"];

/// Lookup a synthetic corpus by name.
///
/// # Errors
///
/// [`CorpusError::UnknownCorpus`] when `name` is not in [`NAMES`].
pub fn by_name(name: &str, size: usize) -> Result<Corpus, CorpusError> {
    let content = generate(name, size)?;
    Ok(Corpus::new(
        format!("synthetic-{name}"),
        vec![CorpusFile::in_memory(
            format!("{name}.bin"),
            content,
        )],
    ))
}

/// Return all synthetic corpora at the requested file size.
#[must_use]
pub fn all(size: usize) -> Vec<Corpus> {
    NAMES
        .iter()
        .filter_map(|&n| by_name(n, size).ok())
        .collect()
}

fn generate(name: &str, size: usize) -> Result<Vec<u8>, CorpusError> {
    match name {
        "zeros" => Ok(vec![0u8; size]),
        "random" => Ok(pseudo_random(size)),
        "text" => Ok(english_text(size)),
        "mixed" => Ok(mixed_payload(size)),
        _ => Err(CorpusError::UnknownCorpus { name: name.to_string() }),
    }
}

/// Deterministic xorshift PRNG — no time source, no external state.
fn pseudo_random(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size);
    let mut state: u64 = 0x0123_4567_89AB_CDEF;
    while out.len() < size {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let bytes = state.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes();
        let take = bytes.len().min(size - out.len());
        out.extend_from_slice(&bytes[..take]);
    }
    out
}

fn english_text(size: usize) -> Vec<u8> {
    let paragraph = b"the quick brown fox jumps over the lazy dog. \
pack my box with five dozen liquor jugs. \
she sells sea shells by the sea shore. \
how vexingly quick daft zebras jump! \
lorem ipsum dolor sit amet, consectetur adipiscing elit. ";
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        out.extend_from_slice(paragraph);
    }
    out.truncate(size);
    out
}

fn mixed_payload(size: usize) -> Vec<u8> {
    // Half text + half pseudo-random — stresses both statistical and
    // dictionary-oriented codecs.
    let half = size / 2;
    let mut out = english_text(half);
    out.extend_from_slice(&pseudo_random(size - half));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zeros_is_all_zero() {
        let c = by_name("zeros", 128).unwrap();
        assert_eq!(c.files()[0].content().len(), 128);
        assert!(c.files()[0].content().iter().all(|&b| b == 0));
    }

    #[test]
    fn random_is_deterministic() {
        let a = by_name("random", 256).unwrap();
        let b = by_name("random", 256).unwrap();
        assert_eq!(a.files()[0].content(), b.files()[0].content());
    }

    #[test]
    fn text_is_non_empty() {
        let c = by_name("text", 100).unwrap();
        assert_eq!(c.files()[0].content().len(), 100);
        assert!(c.files()[0].content().starts_with(b"the quick"));
    }

    #[test]
    fn unknown_name_errors() {
        assert!(by_name("nope", 64).is_err());
    }

    #[test]
    fn mixed_has_both_halves() {
        let c = by_name("mixed", 200).unwrap();
        let content = c.files()[0].content();
        assert_eq!(content.len(), 200);
        // First 100 bytes should be text (printable).
        assert!(content[..100].iter().all(|&b| b.is_ascii_graphic() || b == b' '));
    }
}
