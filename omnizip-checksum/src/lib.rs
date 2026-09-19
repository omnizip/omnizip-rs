//! omnizip-checksum — pluggable checksum family + registry + verifier.
//!
//! Port of the Ruby gem's `ChecksumRegistry` + `checksums/` layer
//! (`TODO.ref-parity/52`): name-keyed registry with duplicate
//! rejection and `available()`, streaming checksums, and a verifier
//! for stored digests. Feeds `ozip verify` (task 55).
//!
//! ## SSOT
//!
//! CRC-32 is NOT reimplemented here — `omnizip-codecs::checksum`
//! owns it (slice-by-8, differential-tested against zlib). This
//! crate wraps it behind the [`Checksum`] trait and adds the
//! CRC-64 family. Adding a checksum = one `register()` call; the
//! registry, verifier, and callers never change (OCP).
//!
//! ## Determinism
//!
//! Identical input ⇒ identical digest; the registry iterates in
//! name order (`BTreeMap`), never hash order.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

/// A finished checksum value.
#[derive(Clone, PartialEq, Eq)]
pub enum Digest {
    /// 32-bit checksums (CRC-32 family).
    U32(u32),
    /// 64-bit checksums (CRC-64 family).
    U64(u64),
}

impl Digest {
    /// The digest as lowercase hex (16 zero-padded digits for U64,
    /// 8 for U32).
    #[must_use]
    pub fn to_hex(&self) -> String {
        match self {
            Self::U32(v) => format!("{v:08x}"),
            Self::U64(v) => format!("{v:016x}"),
        }
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Digest({self})")
    }
}

/// A streaming checksum. `update` may be called any number of
/// times; `finish` returns the digest of the concatenated input.
pub trait Checksum: Send {
    /// Absorb `data`.
    fn update(&mut self, data: &[u8]);
    /// Digest everything absorbed so far (does not reset).
    fn finish(&self) -> Digest;
}

/// Errors from the checksum layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChecksumError {
    /// `create(name)` matched nothing; carries the known names.
    Unknown {
        /// The requested name.
        name: String,
        /// Sorted names the registry knows.
        available: Vec<String>,
    },
    /// `register` hit an existing name.
    Duplicate {
        /// The colliding name.
        name: &'static str,
    },
}

impl fmt::Display for ChecksumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { name, available } => {
                write!(
                    f,
                    "unknown checksum '{name}' (available: {})",
                    available.join(", ")
                )
            }
            Self::Duplicate { name } => write!(f, "checksum '{name}' is already registered"),
        }
    }
}

impl std::error::Error for ChecksumError {}

/// Constructor for a fresh [`Checksum`] instance.
pub type ChecksumFactory = fn() -> Box<dyn Checksum>;

/// Name-keyed checksum registry (the Ruby `ChecksumRegistry`
/// semantics: duplicate registration is an error, `available`
/// lists the families).
///
/// ```
/// use omnizip_checksum::{ChecksumRegistry, Digest};
///
/// let registry = ChecksumRegistry::standard();
/// assert_eq!(
///     registry.compute("crc32", b"123456789").unwrap(),
///     Digest::U32(0xCBF4_3926)
/// );
/// ```
#[derive(Default)]
pub struct ChecksumRegistry {
    factories: BTreeMap<&'static str, ChecksumFactory>,
}

impl ChecksumRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registry with the built-in families: `crc32`, `crc64-xz`,
    /// `crc64-ecma`.
    #[must_use]
    pub fn standard() -> Self {
        let mut r = Self::new();
        r.register("crc32", crc32_checksum).ok();
        r.register("crc64-xz", crc64::Crc64Xz::boxed).ok();
        r.register("crc64-ecma", crc64::Crc64Ecma::boxed).ok();
        r
    }

    /// Register `factory` under `name`. Re-registering a name (even
    /// with the same factory) is an error — mirrors the Ruby
    /// registry's duplicate rejection.
    ///
    /// # Errors
    ///
    /// [`ChecksumError::Duplicate`] when `name` is taken.
    pub fn register(
        &mut self,
        name: &'static str,
        factory: ChecksumFactory,
    ) -> Result<(), ChecksumError> {
        if self.factories.contains_key(name) {
            return Err(ChecksumError::Duplicate { name });
        }
        self.factories.insert(name, factory);
        Ok(())
    }

    /// The registered names, sorted.
    #[must_use]
    pub fn available(&self) -> Vec<String> {
        self.factories.keys().map(|k| (*k).to_string()).collect()
    }

    /// Build a fresh streaming checksum by name.
    ///
    /// # Errors
    ///
    /// [`ChecksumError::Unknown`] listing the available names.
    pub fn create(&self, name: &str) -> Result<Box<dyn Checksum>, ChecksumError> {
        self.factories.get(name).map_or_else(
            || {
                Err(ChecksumError::Unknown {
                    name: name.to_string(),
                    available: self.available(),
                })
            },
            |f| Ok(f()),
        )
    }

    /// One-shot digest of `data` under `name`.
    ///
    /// # Errors
    ///
    /// [`ChecksumError::Unknown`] when the name is not registered.
    pub fn compute(&self, name: &str, data: &[u8]) -> Result<Digest, ChecksumError> {
        let mut c = self.create(name)?;
        c.update(data);
        Ok(c.finish())
    }

    /// Stream-verify: `true` when `data` digests to `expected`
    /// under `name`.
    ///
    /// # Errors
    ///
    /// [`ChecksumError::Unknown`] when the name is not registered.
    pub fn verify(
        &self,
        name: &str,
        data: &[u8],
        expected: &Digest,
    ) -> Result<bool, ChecksumError> {
        Ok(self.compute(name, data)? == *expected)
    }
}

/// CRC-32/ISO-HDLC wrapper over the codecs crate's implementation
/// (SSOT). The incremental state chains through the raw
/// pre-final-XOR value: `crc32_iso_hdlc_update` returns the
/// final-XORed value, so the raw continuation is its bitwise
/// complement (the XOR is an involution). The random-partition
/// tests below prove the chain equals the one-shot.
struct Crc32 {
    raw: u32,
}

impl Crc32 {
    fn new() -> Self {
        Self { raw: 0xFFFF_FFFF }
    }
}

impl Checksum for Crc32 {
    fn update(&mut self, data: &[u8]) {
        self.raw = !omnizip_codecs::checksum::crc32_iso_hdlc_update(self.raw, data);
    }
    fn finish(&self) -> Digest {
        Digest::U32(!self.raw)
    }
}

fn crc32_checksum() -> Box<dyn Checksum> {
    Box::new(Crc32::new())
}

pub mod crc64;

#[cfg(test)]
mod tests {
    use super::*;

    // Deterministic PRNG for the partition fuzz.
    fn next(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn crc32_known_vectors() {
        let r = ChecksumRegistry::standard();
        assert_eq!(r.compute("crc32", b"").unwrap(), Digest::U32(0));
        assert_eq!(
            r.compute("crc32", b"123456789").unwrap(),
            Digest::U32(0xCBF4_3926)
        );
        assert_eq!(
            r.compute("crc32", b"The quick brown fox jumps over the lazy dog")
                .unwrap(),
            Digest::U32(0x414F_A339)
        );
    }

    #[test]
    fn crc64_vectors() {
        let r = ChecksumRegistry::standard();
        assert_eq!(
            r.compute("crc64-xz", b"123456789").unwrap(),
            Digest::U64(0x995D_C9BB_DF19_39FA)
        );
        assert_eq!(r.compute("crc64-xz", b"").unwrap(), Digest::U64(0));
        assert_eq!(
            r.compute("crc64-ecma", b"123456789").unwrap(),
            Digest::U64(0x6C40_DF5F_0B49_7347)
        );
        assert_eq!(r.compute("crc64-ecma", b"").unwrap(), Digest::U64(0));
    }

    #[test]
    fn incremental_partitions_match_one_shot() {
        let mut state = 0xC0FFEE_u64;
        for len in [0_usize, 1, 7, 64, 1000] {
            let data: Vec<u8> = (0..len).map(|_| next(&mut state) as u8).collect();
            let r = ChecksumRegistry::standard();
            for name in r.available() {
                let one_shot = r.compute(&name, &data).unwrap();
                let mut c = r.create(&name).unwrap();
                let mut i = 0;
                while i < data.len() {
                    let n = (next(&mut state) as usize % 37 + 1).min(data.len() - i);
                    c.update(&data[i..i + n]);
                    i += n;
                }
                assert_eq!(c.finish(), one_shot, "{name} len {len}");
            }
        }
    }

    #[test]
    fn registry_semantics_match_ruby() {
        let mut r = ChecksumRegistry::standard();
        let before = r.available();
        assert_eq!(
            r.register("crc32", crc32_checksum),
            Err(ChecksumError::Duplicate { name: "crc32" })
        );
        r.register("crc32-custom", crc32_checksum).unwrap();
        assert_eq!(r.available().len(), before.len() + 1);
        // available() is sorted (BTreeMap) — stable across calls.
        assert_eq!(r.available(), r.available());
        let err = r.create("nope").err().expect("unknown name must error");
        assert!(err.to_string().contains("crc32"));
        assert!(err.to_string().contains("nope"));
        assert_eq!(
            err,
            ChecksumError::Unknown {
                name: "nope".to_string(),
                available: r.available(),
            }
        );
    }

    #[test]
    fn verifier_round_trip() {
        let r = ChecksumRegistry::standard();
        let digest = r.compute("crc64-xz", b"payload").unwrap();
        assert!(r.verify("crc64-xz", b"payload", &digest).unwrap());
        assert!(!r.verify("crc64-xz", b"payloae", &digest).unwrap());
    }

    #[test]
    fn digest_hex_formatting() {
        assert_eq!(Digest::U32(0xCBF4_3926).to_hex(), "cbf43926");
        assert_eq!(
            Digest::U64(0x00FF_00FF_00FF_00FF).to_hex(),
            "00ff00ff00ff00ff"
        );
        assert_eq!(Digest::U32(0).to_string(), "00000000");
    }
}
