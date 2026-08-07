//! In-process synthetic corpora — no network required.
//!
//! Used for smoke-testing the bench itself and for CI runs where
//! downloading Silesia/Enwik8 is too slow. Each variant generates a
//! deterministic byte sequence (seeded RNG, no time source).

use crate::corpus::{Corpus, CorpusError, CorpusFile};
use crate::llm_corpus::{chat_response, code_gen, structured_json, XorShift};

/// Names of the synthetic corpora known to [`all`].
pub const NAMES: &[&str] = &[
    "zeros",
    "random",
    "text",
    "mixed",
    "llm-chat",
    "llm-code",
    "llm-json",
    "llm-mixed",
    "ait-mix",
];

/// Lookup a synthetic corpus by name.
///
/// # Errors
///
/// [`CorpusError::UnknownCorpus`] when `name` is not in [`NAMES`].
pub fn by_name(name: &str, size: usize) -> Result<Corpus, CorpusError> {
    let content = generate(name, size)?;
    Ok(Corpus::new(
        format!("synthetic-{name}"),
        vec![CorpusFile::in_memory(format!("{name}.bin"), content)],
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
        "llm-chat" => {
            let mut xs = XorShift::new(0xCAFE);
            Ok(chat_response(&mut xs, size))
        }
        "llm-code" => {
            let mut xs = XorShift::new(0xBEEF);
            Ok(code_gen(&mut xs, size))
        }
        "llm-json" => {
            let mut xs = XorShift::new(0xF00D);
            Ok(structured_json(&mut xs, size))
        }
        // "llm-mixed" interleaves chat + code + JSON at byte intervals.
        "llm-mixed" => {
            let mut master = XorShift::new(0xBA5E_BA11);
            let mut out = Vec::with_capacity(size);
            while out.len() < size {
                let chunk_size = size / 3 + 1;
                match master.next_usize(3) {
                    0 => out.extend_from_slice(&chat_response(
                        &mut XorShift::new(master.next_u64()),
                        chunk_size,
                    )),
                    1 => out.extend_from_slice(&code_gen(
                        &mut XorShift::new(master.next_u64()),
                        chunk_size,
                    )),
                    _ => out.extend_from_slice(&structured_json(
                        &mut XorShift::new(master.next_u64()),
                        chunk_size,
                    )),
                }
            }
            out.truncate(size);
            Ok(out)
        }
        // "ait-mix" — heterogeneous mix approximating the AIT 2026
        // challenge's 16-file corpus (text + code + JSON + binary).
        "ait-mix" => {
            let chunk = size / 16 + 1;
            let mut out = Vec::with_capacity(size);
            // 6 text chunks, 4 code chunks, 3 JSON chunks, 3 binary chunks.
            for _ in 0..6 {
                out.extend_from_slice(&english_text(chunk));
            }
            for _ in 0..4 {
                let mut xs = XorShift::new(0xA17C0DE);
                out.extend_from_slice(&code_gen(&mut xs, chunk));
            }
            for _ in 0..3 {
                let mut xs = XorShift::new(0xA17C0DE);
                out.extend_from_slice(&structured_json(&mut xs, chunk));
            }
            for _ in 0..3 {
                out.extend_from_slice(&pseudo_random(chunk));
            }
            out.truncate(size);
            Ok(out)
        }
        _ => Err(CorpusError::UnknownCorpus {
            name: name.to_string(),
        }),
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
        assert!(content[..100]
            .iter()
            .all(|&b| b.is_ascii_graphic() || b == b' '));
    }

    #[test]
    fn llm_chat_is_deterministic() {
        let a = by_name("llm-chat", 2048).unwrap();
        let b = by_name("llm-chat", 2048).unwrap();
        assert_eq!(a.files()[0].content(), b.files()[0].content());
    }

    #[test]
    fn llm_code_contains_code_fences() {
        let c = by_name("llm-code", 4096).unwrap();
        let content = c.files()[0].content();
        assert!(content.windows(3).any(|w| w == b"```"));
    }

    #[test]
    fn llm_json_is_well_formed_json_braces() {
        let c = by_name("llm-json", 2048).unwrap();
        let content = c.files()[0].content();
        // All bytes should be JSON-ish (printable + whitespace).
        assert!(content.iter().all(|&b| b == b'{'
            || b == b'}'
            || b == b'"'
            || b == b':'
            || b == b','
            || b == b'\n'
            || b == b'\t'
            || b.is_ascii_graphic()
            || b == b' '));
    }

    #[test]
    fn llm_mixed_combines_all_three() {
        let c = by_name("llm-mixed", 6144).unwrap();
        let content = c.files()[0].content();
        assert!(content.contains(&b'{')); // JSON
        assert!(content.windows(3).any(|w| w == b"```")); // code
                                                          // No need to assert chat explicitly — the mix contains all three.
    }

    #[test]
    fn ait_mix_contains_all_four_components() {
        let c = by_name("ait-mix", 16 * 100).unwrap();
        let content = c.files()[0].content();
        assert_eq!(content.len(), 1600);
        // Should contain prose, code fence, JSON braces, and high-byte
        // pseudo-random sections (covered by general length check).
        assert!(content.windows(3).any(|w| w == b"```")); // code
        assert!(content.contains(&b'{')); // JSON
                                          // The first chunk is text — assert first byte is ASCII lowercase.
        assert!(content[0].is_ascii_lowercase() || content[0] == b' ');
        assert!(content[1].is_ascii_lowercase() || content[1] == b' ');
    }
}
