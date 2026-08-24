//! PAR2 parity archives — TODO.containers task 13: the packet framing
//! (`PAR2\0PKT` + length + MD5-of-set+type+body + set id + type),
//! main / file-description / input-slice-check packets, slice-level
//! verify (MD5 + CRC-64), recovery-slice creation and repair through
//! Reed-Solomon over GF(2^16) (primitive polynomial x^16+x^12+x^5+1,
//! Vandermonde rows over distinct powers of the generator, any n rows
//! invertible).
#![forbid(unsafe_code)]

pub mod crc64;
pub mod packet;
pub mod reedsolomon;
pub mod verify;

use omnizip_archive_core::ArchiveError;

/// 8-byte packet magic.
pub const PACKET_MAGIC: &[u8; 8] = b"PAR2\0PKT";
/// Packet header size.
pub const PACKET_HEADER_SIZE: usize = 64;

/// ASCII packet types (stored reversed on the wire).
pub mod packet_type {
    pub const MAIN: &[u8; 16] = b"PAR 2.0\0Main\0\0\0\0";
    pub const FILE_DESCRIPTION: &[u8; 16] = b"PAR 2.0\0FileDesc";
    pub const IFSC: &[u8; 16] = b"PAR 2.0\0IFSC\0\0\0\0";
    pub const RECOVERY: &[u8; 16] = b"PAR 2.0\0RecvSlic";
}

/// A file tracked by a recovery set.
#[derive(Clone, Debug)]
pub struct TrackedFile {
    pub file_id: [u8; 16],
    pub name: String,
    pub length: u64,
    /// Per-slice (crc64, md5) checks.
    pub slices: Vec<(u64, [u8; 16])>,
}

/// A parsed recovery set.
#[derive(Clone, Debug, Default)]
pub struct RecoverySet {
    pub set_id: [u8; 16],
    /// Slice (block) size in bytes.
    pub block_size: u64,
    pub files: Vec<TrackedFile>,
    /// Recovery exponents present in this volume set, with their data.
    pub recovery: Vec<(u32, Vec<u8>)>,
}

/// Slice a byte buffer into fixed-size blocks (last block zero
/// padded).
#[must_use]
pub fn slice_blocks(data: &[u8], block_size: usize) -> Vec<Vec<u8>> {
    if data.is_empty() {
        return Vec::new();
    }
    data.chunks(block_size)
        .map(|c| {
            let mut b = c.to_vec();
            b.resize(block_size, 0);
            b
        })
        .collect()
}

/// The 16-byte file id: MD5 of (hash16 ‖ hash ‖ length-le ‖ name).
#[must_use]
pub fn file_id(hash16: &[u8; 16], hash: &[u8; 16], length: u64, name: &str) -> [u8; 16] {
    let mut material = Vec::with_capacity(16 + 16 + 8 + name.len());
    material.extend_from_slice(hash16);
    material.extend_from_slice(hash);
    material.extend_from_slice(&length.to_le_bytes());
    material.extend_from_slice(name.as_bytes());
    omnizip_crypto::md5(&material)
}

/// Errors for the public API funnel through [`ArchiveError`].
pub(crate) fn invalid(reason: impl Into<String>) -> ArchiveError {
    ArchiveError::InvalidArchive(reason.into())
}
