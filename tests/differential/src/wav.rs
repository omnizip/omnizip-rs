//! WAV header construction for differential tests.
//!
//! Single source of truth for the 44-byte canonical PCM WAV header.
//! Extracted from `tests/differential/tests/flac_parity.rs` so future
//! parity tests can construct WAV fixtures without duplicating the
//! struct packing.

#![forbid(unsafe_code)]

/// Build a canonical 44-byte-header WAV byte vector for `n` mono
/// 16-bit samples produced by the closure `f(i)`.
///
/// The resulting WAV uses the given `sample_rate` for both the sample
/// rate field and the byte-rate field (sample_rate × 2 for 16-bit mono).
/// It is suitable for piping into a FLAC encoder that consumes WAV
/// input or for byte-comparison with a libFLAC-decoded WAV.
#[must_use]
pub fn mono<F: Fn(usize) -> i16>(n: usize, sample_rate: u32, f: F) -> Vec<u8> {
    let data: Vec<u8> = (0..n).flat_map(|i| f(i).to_le_bytes()).collect();
    let mut hdr = Vec::with_capacity(44 + data.len());
    hdr.extend_from_slice(b"RIFF");
    hdr.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    hdr.extend_from_slice(b"WAVEfmt ");
    hdr.extend_from_slice(&16u32.to_le_bytes());
    hdr.extend_from_slice(&1u16.to_le_bytes()); // PCM
    hdr.extend_from_slice(&1u16.to_le_bytes()); // mono
    hdr.extend_from_slice(&sample_rate.to_le_bytes());
    hdr.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    hdr.extend_from_slice(&2u16.to_le_bytes()); // block align
    hdr.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    hdr.extend_from_slice(b"data");
    hdr.extend_from_slice(&(data.len() as u32).to_le_bytes());
    hdr.extend_from_slice(&data);
    hdr
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mono_header_is_44_bytes_plus_payload() {
        let wav = mono(10, 8000, |_| 0);
        assert_eq!(wav.len(), 44 + 20);
    }

    #[test]
    fn mono_writes_canonical_riff_wave_markers() {
        let wav = mono(1, 8000, |_| 0);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
    }

    #[test]
    fn mono_stores_sample_rate_at_offset_24_le() {
        let wav = mono(1, 44_100, |_| 0);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 44_100);
    }
}
