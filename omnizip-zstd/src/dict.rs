//! ZSTD dictionary — wire format + (de)serialization.
//!
//! A ZSTD dictionary lets the encoder preload entropy tables and a
//! reference-content window so that small inputs (< 100 KB) compress
//! dramatically better. The match finder can reference dictionary
//! content as if it were earlier in the stream.
//!
//! ## Wire format (RFC 8878 §5)
//!
//! ```text
//! Magic: \x37\xA4\x30\xEC (4 bytes, LE = 0xEC30A437)
//! Dictionary_ID: u32 LE (4 bytes)
//! Entropy tables:   Huffman weights, FSE LL/ML/OF, repeat offsets
//! Content:          Raw sample bytes (for match finding)
//! ```
//!
//! Phase 1 uses a **simplified** format: magic + ID + raw content.
//! Entropy-table preloading lands in Phase C alongside the FSE encoder
//! port (see `TODO.omnizip-rs/15-zstd-phase-c-fse.md`). The simplified
//! form round-trips through this crate's own (de)serializer and is
//! sufficient for the dictionary-prefix match-finder path.

#![forbid(unsafe_code)]

use crate::ZstdError;

/// ZSTD dictionary magic — RFC 8878 §5.1.
pub const DICT_MAGIC: u32 = 0xEC30_A437;
/// Little-endian byte sequence of [`DICT_MAGIC`].
pub const DICT_MAGIC_BYTES: [u8; 4] = [0x37, 0xA4, 0x30, 0xEC];

/// A ZSTD dictionary. Holds a `Dictionary_ID` and a content blob.
///
/// Phase 1 dictionaries carry only raw content — entropy tables are
/// deferred to Phase C. The content is used as a prefix to the input
/// during compression so the match finder can reference it.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ZstdDictionary {
    id: u32,
    content: Vec<u8>,
}

impl ZstdDictionary {
    /// Wrap raw bytes as a dictionary with the given ID. No entropy
    /// tables are attached; the content is used as a match-finder
    /// prefix during compression.
    #[must_use]
    pub fn from_raw(id: u32, content: &[u8]) -> Self {
        Self {
            id,
            content: content.to_vec(),
        }
    }

    /// The dictionary's numeric ID (matches the frame header
    /// `Dictionary_ID` field when this dictionary is in use).
    #[must_use]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Raw content bytes — the corpus material the match finder can
    /// reference as a prefix to the input.
    #[must_use]
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// Deserialize a Phase 1 dictionary blob: magic + ID + content.
    ///
    /// # Errors
    ///
    /// Returns [`ZstdError::Corrupt`] if the magic doesn't match or
    /// the blob is truncated.
    pub fn deserialize(data: &[u8]) -> Result<Self, ZstdError> {
        if data.len() < DICT_MAGIC_BYTES.len() + 4 {
            return Err(ZstdError::Corrupt {
                reason: "dictionary too short for magic + id".into(),
            });
        }
        if data[..DICT_MAGIC_BYTES.len()] != DICT_MAGIC_BYTES {
            return Err(ZstdError::Corrupt {
                reason: format!(
                    "bad dictionary magic: got {:02X?}, expected {:02X?}",
                    &data[..DICT_MAGIC_BYTES.len()],
                    DICT_MAGIC_BYTES
                ),
            });
        }
        let id = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let content = data[8..].to_vec();
        Ok(Self { id, content })
    }

    /// Serialize to a Phase 1 dictionary blob: magic + ID + content.
    #[must_use]
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.content.len());
        out.extend_from_slice(&DICT_MAGIC_BYTES);
        out.extend_from_slice(&self.id.to_le_bytes());
        out.extend_from_slice(&self.content);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_raw_stores_id_and_content() {
        let d = ZstdDictionary::from_raw(42, b"hello");
        assert_eq!(d.id(), 42);
        assert_eq!(d.content(), b"hello");
    }

    #[test]
    fn serialize_round_trips() {
        let d = ZstdDictionary::from_raw(0x1234_5678, b"corpus bytes");
        let blob = d.serialize();
        let d2 = ZstdDictionary::deserialize(&blob).expect("deserialize");
        assert_eq!(d, d2);
    }

    #[test]
    fn serialize_starts_with_magic() {
        let d = ZstdDictionary::from_raw(1, b"x");
        let blob = d.serialize();
        assert_eq!(&blob[..4], &DICT_MAGIC_BYTES);
    }

    #[test]
    fn deserialize_rejects_bad_magic() {
        let mut bad = DICT_MAGIC_BYTES.to_vec();
        bad[0] ^= 0xFF;
        bad.extend_from_slice(&1u32.to_le_bytes());
        let err = ZstdDictionary::deserialize(&bad).unwrap_err();
        assert!(matches!(err, ZstdError::Corrupt { .. }));
    }

    #[test]
    fn deserialize_rejects_short_input() {
        let err = ZstdDictionary::deserialize(&[0x37, 0xA4]).unwrap_err();
        assert!(matches!(err, ZstdError::Corrupt { .. }));
    }

    #[test]
    fn empty_content_round_trips() {
        let d = ZstdDictionary::from_raw(99, b"");
        let blob = d.serialize();
        // magic (4) + id (4) + 0 content.
        assert_eq!(blob.len(), 8);
        let d2 = ZstdDictionary::deserialize(&blob).expect("deserialize");
        assert_eq!(d, d2);
    }
}
