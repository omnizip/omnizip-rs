//! Move-to-Front (MTF) transform.
//!
//! Port of `omnizip/lib/omnizip/algorithms/bzip2/mtf.rb`.
//!
//! Maintains a list of all 256 byte values. For each input byte, output its
//! current position in the list, then move that byte to the front. After BWT
//! the data tends to have long runs of one value, which MTF turns into runs
//! of small numbers (often 0) — ideal input for the subsequent RLE stage.

/// Encode `data` using MTF.
///
/// # Panics
///
/// Panics if a byte value cannot be found in the symbol list, which can only
/// happen for input outside the `0..=255` byte range (impossible for `&[u8]`).
#[must_use]
pub fn mtf_encode(data: &[u8]) -> Vec<u8> {
    let mut symbols: Vec<u8> = (0..=255).collect();
    let mut result = Vec::with_capacity(data.len());

    for &byte in data {
        let pos = symbols
            .iter()
            .position(|&s| s == byte)
            .expect("byte value is always in 0..=255");
        result.push(pos as u8);
        symbols.remove(pos);
        symbols.insert(0, byte);
    }
    result
}

/// Decode MTF-encoded `data` back to the original bytes.
#[must_use]
pub fn mtf_decode(data: &[u8]) -> Vec<u8> {
    let mut symbols: Vec<u8> = (0..=255).collect();
    let mut result = Vec::with_capacity(data.len());

    for &index in data {
        let byte = symbols[index as usize];
        result.push(byte);
        symbols.remove(index as usize);
        symbols.insert(0, byte);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_round_trips() {
        let enc = mtf_encode(b"");
        assert!(enc.is_empty());
        let dec = mtf_decode(&enc);
        assert!(dec.is_empty());
    }

    #[test]
    fn round_trip_simple() {
        let data = b"banana";
        let enc = mtf_encode(data);
        let dec = mtf_decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trip_text() {
        let data = b"The quick brown fox jumps over the lazy dog.";
        let enc = mtf_encode(data);
        let dec = mtf_decode(&enc);
        assert_eq!(dec, data);
    }

    #[test]
    fn repetitive_input_yields_low_indices() {
        // After the first occurrence, repeats of the same byte should all
        // encode to 0 (it's already at the front).
        let data = b"aaaaa";
        let enc = mtf_encode(data);
        assert_eq!(enc[0], b'a');
        assert!(enc[1..].iter().all(|&v| v == 0));
    }

    #[test]
    fn round_trip_all_bytes() {
        let data: Vec<u8> = (0..=255).collect::<Vec<u8>>().repeat(3);
        let enc = mtf_encode(&data);
        let dec = mtf_decode(&enc);
        assert_eq!(dec, data);
    }
}
