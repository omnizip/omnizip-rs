//! Literal context computation (RFC 7932 §10.1).
//!
//! Each literal byte is encoded using a Huffman tree selected by a
//! context ID derived from the previous 1–2 bytes. The context
//! function depends on CONTEXT_MODE.

use crate::static_codes::K_UTF8_CONTEXT_LOOKUP;

/// Check if input looks like text (printable ASCII + whitespace).
/// Used to select UTF8 context mode over LSB6 for better ratio.
pub fn is_text_like(input: &[u8]) -> bool {
    if input.is_empty() {
        return false;
    }
    let text_bytes = input.iter().filter(|&&b| is_text_byte(b)).count();
    text_bytes * 10 > input.len() * 9 // > 90% text bytes
}

/// A byte is "text-like" if it's printable ASCII or common whitespace.
fn is_text_byte(b: u8) -> bool {
    matches!(b, 0x09 | 0x0A | 0x0D) || (0x20..=0x7E).contains(&b)
}

/// Compute a literal context ID (RFC 7932 §10.1) for the given mode.
///
/// - `mode == 0` (LSB6): `p1 & 0x3F` (6-bit context from previous byte)
/// - `mode == 2` (UTF8): lookup-table-based context separating UTF-8
///   character classes
///
/// MSB6 (1) and Signed (3) are not used by the encoder but documented
/// for completeness.
pub fn compute_context_id(p1: u8, p2: u8, mode: u32) -> u8 {
    match mode {
        0 => p1 & 0x3F, // LSB6
        2 => K_UTF8_CONTEXT_LOOKUP[p1 as usize] | K_UTF8_CONTEXT_LOOKUP[(p2 as usize) | 256],
        _ => p1 & 0x3F, // fallback to LSB6
    }
}
