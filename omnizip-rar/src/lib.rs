//! RAR archives — TODO.containers tasks 07/08: RAR5 (VINT block
//! headers, STORE-method read + deterministic write) and RAR4
//! (read-only, marker/file/comment/end blocks, STORE extraction).
//! Both verify CRC32 on extraction. LZ-compressed entries surface a
//! clear UnsupportedFeature (the Ruby reference shells out to unrar
//! for those); STORE archives interoperate with real unrar in both
//! directions.
#![forbid(unsafe_code)]

pub mod rar3;
pub mod rar5;

/// RAR4 signature: `Rar!\x1A\x07\x00`.
pub const MAGIC_RAR4: [u8; 7] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x00];
/// RAR5 signature: `Rar!\x1A\x07\x01\x00`.
pub const MAGIC_RAR5: [u8; 8] = [0x52, 0x61, 0x72, 0x21, 0x1A, 0x07, 0x01, 0x00];

/// RAR5 block types.
pub mod rar5_block {
    pub const MAIN: u64 = 1;
    pub const FILE: u64 = 2;
    pub const SERVICE: u64 = 3;
    pub const ENCRYPTION: u64 = 4;
    pub const END: u64 = 5;
}

/// RAR5 header flags.
pub mod rar5_header_flags {
    pub const EXTRA_AREA: u64 = 0x0001;
    pub const DATA_AREA: u64 = 0x0002;
    pub const SKIP_IF_UNKNOWN: u64 = 0x0004;
    pub const DATA_AREA_SIZE_UNKNOWN: u64 = 0x0008;
    pub const ENCRYPTED: u64 = 0x0010;
}

/// RAR5 file flags.
pub mod rar5_file_flags {
    pub const IS_DIR: u64 = 0x0001;
    pub const TIME_PRESENT: u64 = 0x0002;
    pub const CRC32_PRESENT: u64 = 0x0004;
    pub const UNPACKED_UNKNOWN: u64 = 0x0008;
}

/// Host OS codes.
pub mod host_os {
    pub const WINDOWS: u64 = 0;
    pub const UNIX: u64 = 1;
}

/// RAR5 compression-info layout (validated against unrar-produced
/// archives via 7-Zip listings): bits 0..5 version, bit 6 solid,
/// bits 7..9 method (0 = store, 1..5 = LZ), bit 10+ dictionary code
/// (0 = 128 KB, +1 per doubling).
#[must_use]
pub fn rar5_comp_info(method: u64, solid: bool) -> u64 {
    let m = method.clamp(0, 5) << 7;
    m | u64::from(solid) << 6
}

/// Decode a RAR5 compression-info vint into (version, solid, method,
/// dict code).
#[must_use]
pub fn rar5_decode_comp_info(ci: u64) -> (u64, bool, u64, u64) {
    (ci & 0x3F, ci & 0x40 != 0, (ci >> 7) & 0x7, ci >> 10)
}

/// Encode a RAR5 VINT (7 bits per byte, little-endian, high bit = more).
pub fn write_vint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// VINT byte length for `value`.
#[must_use]
pub fn vint_len(value: u64) -> usize {
    let mut n = 1;
    let mut v = value >> 7;
    while v != 0 {
        n += 1;
        v >>= 7;
    }
    n
}

/// RAR4 block types (HEAD_TYPE byte).
pub mod rar4_block {
    pub const MARKER: u8 = 0x72;
    pub const ARCHIVE: u8 = 0x73;
    pub const FILE: u8 = 0x74;
    pub const COMMENT: u8 = 0x75;
    pub const OLD_EXTRA: u8 = 0x76;
    pub const OLD_SUBBLOCK: u8 = 0x77;
    pub const OLD_RECOVERY: u8 = 0x78;
    pub const OLD_AUTH: u8 = 0x79;
    pub const SUBBLOCK: u8 = 0x7A;
    pub const END: u8 = 0x7B;
}

/// RAR4 file-header flags (subset).
pub mod rar4_flags {
    pub const SPLIT_BEFORE: u16 = 0x0001;
    pub const SPLIT_AFTER: u16 = 0x0002;
    pub const ENCRYPTED: u16 = 0x0004;
    pub const SOLID: u16 = 0x0010;
    pub const DIRECTORY: u16 = 0x00E0;
    pub const LARGE: u16 = 0x0100;
    pub const UNICODE: u16 = 0x0200;
    pub const SALT: u16 = 0x0400;
}

/// RAR4 methods: 0x30 = store, 0x31..0x35 LZ, 0x40+ delta-audio.
pub const RAR4_METHOD_STORE: u8 = 0x30;
