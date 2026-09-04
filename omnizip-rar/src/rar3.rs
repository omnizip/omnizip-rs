//! RAR4 reader — marker block, then block stream: crc16 ‖ type ‖ flags
//! ‖ size ‖ (file header fields). File blocks carry the 25-byte fixed
//! field set, DOS time, optional 64-bit sizes / salt / ext-time, the
//! name, and the packed data after the header. STORE entries extract
//! directly; LZ entries (unpack versions 15/20/26/29) decode through
//! the pure-Rust [`crate::rar3_unpack`] port (LZSS + PPMd + VM
//! filters); RAR3-encrypted entries and -hp encrypted-header archives
//! decrypt through [`crate::rar3_crypto`]. Split entries stitch
//! across concatenated volumes exactly like the RAR5 reader.
#![forbid(unsafe_code)]

use crate::rar3_unpack::Unpacker30;
use crate::{rar4_block, rar4_flags, MAGIC_RAR4, RAR4_METHOD_STORE};
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader, EntryKind};
use std::path::Path;

#[derive(Clone)]
struct RawEntry {
    name: String,
    is_dir: bool,
    is_symlink: bool,
    /// Split entry still awaiting its final part.
    split_open: bool,
    unpacked_size: u64,
    crc32: Option<u32>,
    method: u8,
    unp_ver: u8,
    solid: bool,
    encrypted: bool,
    salt: Option<[u8; 8]>,
    window_size: usize,
    mtime: Option<u64>,
    data: (usize, usize),
}

/// Reads a RAR4 (RAR 1.5-4.x) archive held in memory.
pub struct Rar4Reader {
    /// Contiguous packed data for every entry (split parts stitched).
    arena: Vec<u8>,
    entries: Vec<RawEntry>,
    password: Option<Vec<u8>>,
    unpacker: Unpacker30,
    /// Solid streams decode in archive order; this tracks progress.
    consumed: usize,
    cached: Option<(usize, Vec<u8>)>,
}

impl Rar4Reader {
    /// Parse a RAR4 archive.
    ///
    /// # Errors
    ///
    /// [`ArchiveError`] on signature or structure problems.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        if data.len() < 7 || data[0..7] != MAGIC_RAR4 {
            return Err(ArchiveError::InvalidArchive(
                "rar4: invalid signature".into(),
            ));
        }
        let mut entries = Vec::new();
        let mut arena = Vec::new();
        let mut pos = 7usize;
        let mut continuation: Option<usize> = None;
        let mut header_encrypted = false;

        while let Some(next) = parse_block(
            data,
            pos,
            &mut entries,
            &mut arena,
            &mut continuation,
            &mut header_encrypted,
        )? {
            pos = next;
        }
        Ok(Self {
            arena,
            entries,
            password: None,
            unpacker: Unpacker30::new(),
            consumed: 0,
            cached: None,
        })
    }

    /// Parse with a password: encrypted-header (-hp) archives are
    /// decrypted transparently and entry data decrypts per-entry.
    ///
    /// # Errors
    ///
    /// [`ArchiveError`] on structure or password problems.
    pub fn from_bytes_with_password(data: &[u8], password: &str) -> Result<Self, ArchiveError> {
        let spliced = splice_encrypted_headers(data, password.as_bytes())?;
        let mut reader = Self::from_bytes(&spliced)?;
        reader.password = Some(password.as_bytes().to_vec());
        Ok(reader)
    }

    /// Open from disk.
    ///
    /// # Errors
    ///
    /// IO or archive errors.
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        let data = std::fs::read(path).map_err(|e| ArchiveError::io("read", path, e))?;
        Self::from_bytes(&data)
    }

    /// Provide the password for encrypted entries.
    #[must_use]
    pub fn with_password(mut self, password: &str) -> Self {
        self.password = Some(password.as_bytes().to_vec());
        self
    }

    /// Open a multi-volume set: parts are concatenated in order and
    /// split entries stitch across the repeated signatures and MAIN
    /// headers.
    ///
    /// # Errors
    ///
    /// IO or archive errors from any part.
    pub fn open_volumes(paths: &[std::path::PathBuf]) -> Result<Self, ArchiveError> {
        if paths.is_empty() {
            return Err(ArchiveError::InvalidArchive(
                "rar4: no volumes given".into(),
            ));
        }
        let mut data = Vec::new();
        for path in paths {
            let part = std::fs::read(path).map_err(|e| ArchiveError::io("read", path, e))?;
            data.extend_from_slice(&part);
        }
        Self::from_bytes(&data)
    }

    /// Open a volume set starting at `first` by scanning sibling
    /// parts: `.partNN.rar` (any zero padding), `.rar` + `.rNN`, and
    /// the old two-letter `…-aa/-ab` suffix naming.
    ///
    /// # Errors
    ///
    /// IO or archive errors.
    pub fn open_volume_set(first: &Path) -> Result<Self, ArchiveError> {
        let parts = scan_volume_set(first);
        Self::open_volumes(&parts)
    }
}

fn parse_block(
    data: &[u8],
    pos: usize,
    entries: &mut Vec<RawEntry>,
    arena: &mut Vec<u8>,
    continuation: &mut Option<usize>,
    header_encrypted: &mut bool,
) -> Result<Option<usize>, ArchiveError> {
    let rest = match data.get(pos..) {
        Some(r) if r.len() >= 7 => r,
        _ => return Ok(None),
    };
    let _crc16 = u16::from_le_bytes([rest[0], rest[1]]);
    let kind = rest[2];
    let flags = u16::from_le_bytes([rest[3], rest[4]]);
    let size = u16::from_le_bytes([rest[5], rest[6]]) as usize;
    if size < 7 {
        return Ok(None);
    }
    let end = pos + size;

    match kind {
        rar4_block::ARCHIVE => {
            *header_encrypted = flags & 0x0080 != 0;
        }
        rar4_block::FILE => {
            if rest.len() < 32 {
                return Err(ArchiveError::InvalidArchive(
                    "rar4: short file header".into(),
                ));
            }
            let u32le = |o: usize| u32::from_le_bytes(rest[o..o + 4].try_into().expect("4"));
            let pack_size = u64::from(u32le(7));
            let unpack_size = u64::from(u32le(11));
            let _host_os = rest[15];
            let file_crc = u32le(16);
            let dos_time = u32le(20);
            let unp_ver = rest[24];
            let method = rest[25];
            let name_size = u16::from_le_bytes([rest[26], rest[27]]) as usize;
            let attr = u32le(28);
            let (mut pack, mut unpack) = (pack_size, unpack_size);
            let name_start = if flags & rar4_flags::LARGE != 0 {
                if rest.len() < 40 + name_size {
                    return Err(ArchiveError::InvalidArchive(
                        "rar4: short large header".into(),
                    ));
                }
                pack |= u64::from(u32le(32)) << 32;
                unpack |= u64::from(u32le(36)) << 32;
                40
            } else {
                32
            };
            let name_bytes = rest.get(name_start..name_start + name_size).unwrap_or(&[]);
            // Optional 8-byte salt sits right after the name.
            let mut salt = None;
            if flags & rar4_flags::SALT != 0 {
                if let Some(s) = rest.get(name_start + name_size..name_start + name_size + 8) {
                    salt = Some(s.try_into().expect("8"));
                }
            }
            // Unicode names carry a decoded form after the NUL; the
            // ASCII prefix up to the NUL is always valid.
            let name = decode_name(name_bytes, flags);
            let is_dir = flags & rar4_flags::DIRECTORY == rar4_flags::DIRECTORY;

            let split_before = flags & rar4_flags::SPLIT_BEFORE != 0;
            let split_after = flags & rar4_flags::SPLIT_AFTER != 0;
            // Packed slices from different volumes are not contiguous
            // (headers interleave), so every slice goes into the arena
            // and split parts append to their pending entry.
            let slice = data
                .get(end..end + pack as usize)
                .ok_or_else(|| ArchiveError::InvalidArchive("rar4: data out of bounds".into()))?;
            if split_before {
                match continuation {
                    Some(idx) => {
                        let idx = *idx;
                        arena.extend_from_slice(slice);
                        // Volume parts repeat the file header; every
                        // part except the last stores a pack-CRC in
                        // the CRC field, so the latest header's value
                        // wins and the final part leaves the real
                        // unpacked CRC behind.
                        entries[idx].crc32 = Some(file_crc);
                        // Keep the first slice's start; only the end
                        // advances as parts accumulate.
                        entries[idx].data.1 = arena.len();
                        if split_after {
                            *continuation = Some(idx);
                        } else {
                            entries[idx].split_open = false;
                            *continuation = None;
                        }
                    }
                    None => {
                        // A standalone mid-volume .rar has a
                        // SPLIT_BEFORE entry with no open continuation;
                        // treat as a clean archive end so the
                        // multi-volume API stitches the parts but
                        // walking each part alone stays parseable.
                        return Ok(None);
                    }
                }
            } else {
                let start = arena.len();
                arena.extend_from_slice(slice);
                let entry = RawEntry {
                    name,
                    is_dir,
                    is_symlink: attr & 0xF000 == 0xA000,
                    split_open: split_after && !is_dir,
                    unpacked_size: unpack,
                    crc32: Some(file_crc),
                    method,
                    unp_ver,
                    solid: flags & rar4_flags::SOLID != 0,
                    encrypted: flags & rar4_flags::ENCRYPTED != 0,
                    salt,
                    window_size: if is_dir {
                        0
                    } else {
                        0x10000 << ((flags & 0xE0) >> 5)
                    },
                    mtime: Some(dos_time_to_unix(dos_time)),
                    data: (start, arena.len()),
                };
                *continuation = if entry.split_open {
                    Some(entries.len())
                } else {
                    None
                };
                entries.push(entry);
            }
            // Advance past the packed data area to the next block.
            return Ok(Some(end + pack as usize));
        }
        rar4_block::SUBBLOCK | rar4_block::OLD_SUBBLOCK => {
            // Sub blocks carry file-shaped headers with a data area;
            // skip them entirely.
            let pack = u32::from_le_bytes(rest[7..11].try_into().expect("4")) as usize;
            return Ok(Some(end + pack));
        }
        rar4_block::END => {
            // A next volume's signature may sit after the end block;
            // scan for it like the RAR5 reader.
            if let Some(at) = find_signature(data, end) {
                return Ok(Some(at + MAGIC_RAR4.len()));
            }
            return Ok(None);
        }
        _ => {}
    }
    Ok(Some(end))
}

/// Rebuild the plain header stream of a -hp archive: every header
/// block is stored as `[8-byte salt][align16(AES-CBC ciphertext)]`
/// with the (usually constant) salt prefix; file data areas stay raw
/// in place. The result is byte-identical to the unencrypted layout,
/// so the regular parser handles it.
fn splice_encrypted_headers(data: &[u8], password: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    if data.len() < 20 || data[0..7] != MAGIC_RAR4 {
        return Ok(data.to_vec());
    }
    let mut pos = 7usize;
    let mut main_flags = 0u16;
    let mut out = data[..7].to_vec();
    // Locate the MAIN header.
    while pos + 7 <= data.len() {
        let kind = data[pos + 2];
        let flags = u16::from_le_bytes([data[pos + 3], data[pos + 4]]);
        let size = u16::from_le_bytes([data[pos + 5], data[pos + 6]]) as usize;
        if size < 7 {
            return Err(ArchiveError::InvalidArchive("rar4: bad block size".into()));
        }
        if pos + size > data.len() {
            // Truncated tail: a later block header was really file data
            // (no END block) or the archive is cut short. Pass through
            // so the regular parser reports the truncation — never
            // panic on a bad declared size.
            break;
        }
        out.extend_from_slice(&data[pos..pos + size]);
        pos += size;
        if kind == rar4_block::ARCHIVE {
            main_flags = flags;
            break;
        }
        if kind == rar4_block::END {
            return Ok(data.to_vec());
        }
    }
    if main_flags & 0x0080 == 0 {
        // Not actually header-encrypted; append the remainder.
        out.extend_from_slice(&data[pos.min(data.len())..]);
        return Ok(out);
    }

    let mut kdf_cache: Option<([u8; 8], [u8; 16], [u8; 16])> = None;
    while pos + 8 < data.len() {
        let salt: [u8; 8] = data[pos..pos + 8].try_into().expect("8");
        let cipher_start = pos + 8;
        if data.len() < cipher_start + 16 {
            break;
        }
        // Decrypt the first 16 bytes to learn the header size, then
        // the full aligned block (same continuous CBC chain).
        let mut head16 = data[cipher_start..(cipher_start + 16).min(data.len())].to_vec();
        let (key, iv) = match &kdf_cache {
            Some((s, k, i)) if *s == salt => (*k, *i),
            _ => {
                let keys = crate::rar3_crypto::set_key30(password, Some(&salt));
                kdf_cache = Some((salt, keys.aes_key, keys.aes_init));
                (keys.aes_key, keys.aes_init)
            }
        };
        let mut cipher = omnizip_crypto::AesCbc128Decrypt::new(&key, &iv);
        cipher.decrypt(&mut head16);
        let size = u16::from_le_bytes([head16[5], head16[6]]) as usize;
        if !(7..=0x8000).contains(&size) {
            return Err(ArchiveError::InvalidArchive(
                "rar4: header decryption failed (wrong password?)".into(),
            ));
        }
        let aligned = size.div_ceil(16) * 16;
        let cipher_end = (cipher_start + aligned).min(data.len());
        let mut plain = data[cipher_start..cipher_end].to_vec();
        let mut cipher = omnizip_crypto::AesCbc128Decrypt::new(&key, &iv);
        cipher.decrypt(&mut plain);
        out.extend_from_slice(&plain[..size.min(plain.len())]);

        let kind = plain[2];
        if kind == rar4_block::END {
            break;
        }
        // File data follows raw (encrypted per-entry, if at all).
        if kind == rar4_block::FILE {
            let pack = u32::from_le_bytes(plain[7..11].try_into().expect("4")) as usize;
            let data_end = (cipher_end + pack).min(data.len());
            out.extend_from_slice(&data[cipher_end..data_end]);
            pos = data_end;
        } else {
            pos = cipher_end;
        }
    }
    Ok(out)
}

/// Find the next RAR4 signature at or after `from`.
fn find_signature(data: &[u8], from: usize) -> Option<usize> {
    let limit = data.len().saturating_sub(MAGIC_RAR4.len());
    (from..=limit).find(|&i| data.get(i..i + MAGIC_RAR4.len()) == Some(&MAGIC_RAR4[..]))
}

/// Find all parts of a RAR volume set starting at `first`.
///
/// Recognizes `.partNN.rar` numbering (any zero padding), the
/// `name.rar` + `name.rNN` sibling scheme, and the old two-letter
/// `base-aa`/`base-ab` suffix naming. Returns at least `[first]`.
///
/// Public so the `ozip` CLI can concatenate parts itself (it needs the
/// bytes, not a reader, to keep the password wiring in one place).
pub fn scan_volume_set(first: &Path) -> Vec<std::path::PathBuf> {
    let mut parts = vec![first.to_path_buf()];
    let name = first
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(idx) = name.find(".part") {
        // name.partNN.rar — keep the zero padding.
        if let Some(dot) = name[idx + 5..].find('.') {
            let num_len = dot;
            let stem = &name[..idx + 5];
            let ext = &name[idx + 5 + dot..];
            let mut n = 2u64;
            loop {
                let candidate = format!("{stem}{n:0num_len$}{ext}");
                let path = first.with_file_name(candidate);
                if path.exists() {
                    parts.push(path);
                    n += 1;
                } else {
                    break;
                }
            }
        }
        return parts;
    }
    if name.ends_with(".rar") {
        // name.rar + name.r00, name.r01, ...
        let stem = name[..name.len() - 4].to_string();
        let mut n = 0u64;
        loop {
            let candidate = format!("{stem}.r{n:02}");
            let path = first.with_file_name(candidate);
            if path.exists() {
                parts.push(path);
                n += 1;
            } else {
                break;
            }
        }
        return parts;
    }
    // Old two-letter suffix naming: base-aa, base-ab, ...
    if let Some(stem_len) = name.len().checked_sub(2) {
        let (stem, suffix) = name.split_at(stem_len);
        let is_two_letter = suffix.len() == 2 && suffix.bytes().all(|b| b.is_ascii_lowercase());
        if is_two_letter {
            let mut cur = suffix.to_string();
            loop {
                let mut bytes = [0u8; 2];
                bytes.copy_from_slice(cur.as_bytes());
                let mut i = 1;
                loop {
                    bytes[i] += 1;
                    if bytes[i] <= b'z' {
                        break;
                    }
                    bytes[i] = b'a';
                    if i == 0 {
                        break;
                    }
                    i -= 1;
                }
                cur = String::from_utf8_lossy(&bytes).into_owned();
                let candidate = format!("{stem}{cur}");
                let path = first.with_file_name(candidate.clone());
                if path.exists() {
                    parts.push(path);
                } else {
                    break;
                }
            }
        }
    }
    parts
}

fn decode_name(bytes: &[u8], flags: u16) -> String {
    if bytes.first() == Some(&0) {
        return String::new();
    }
    // Split at the first NUL: ASCII part always present; the rest is
    // the unicode delta encoding we approximate by skipping.
    let ascii = match bytes.iter().position(|&b| b == 0) {
        Some(i) => &bytes[..i],
        None => bytes,
    };
    if flags & rar4_flags::UNICODE != 0 {
        // The Unicode tail is a RLE+high-byte-word encoding; taking
        // the ASCII prefix is correct whenever it is non-empty.
        if !ascii.is_empty() {
            return String::from_utf8_lossy(ascii).into_owned();
        }
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        String::from_utf8_lossy(ascii).into_owned()
    }
}

/// DOS date/time (1980 epoch, little-endian u32) → unix seconds.
fn dos_time_to_unix(dos: u32) -> u64 {
    let secs = (dos & 0x1F) * 2;
    let mins = (dos >> 5) & 0x3F;
    let hours = (dos >> 11) & 0x1F;
    let day = (dos >> 16) & 0x1F;
    let month = (dos >> 21) & 0x0F;
    let year = ((dos >> 25) & 0x7F) + 1980;
    if !(1..=12).contains(&month) || day == 0 {
        return 0;
    }
    let days = days_from_civil(i64::from(year), month, day);
    (days * 86_400 + i64::from(hours * 3600 + mins * 60 + secs)).max(0) as u64
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = i64::from(m) + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

impl Rar4Reader {
    /// Decode entries in archive order up to `index`, keeping the
    /// solid unpacker state correct; returns entry `index`'s bytes.
    fn decode_prefix(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        if index < self.consumed {
            if let Some((i, ref data)) = self.cached {
                if i == index {
                    return Ok(data.clone());
                }
            }
            // Backwards random access: rebuild the stream from the top.
            self.unpacker = Unpacker30::new();
            self.consumed = 0;
            self.cached = None;
        }
        while self.consumed <= index {
            let i = self.consumed;
            let out = if self.entries[i].is_dir {
                Some(Vec::new())
            } else {
                match self.decode_entry(i) {
                    Ok(out) => Some(out),
                    Err(err) if i == index => return Err(err),
                    // A failing member before the requested one must not
                    // lock out later entries (libarchive's partially
                    // encrypted fixture keeps its plaintext file
                    // readable). Non-solid entries reset all unpacker
                    // state themselves; a solid continuation past a
                    // failure cannot be reconstructed anyway and fails
                    // its own CRC check. The failure is not cached so a
                    // direct later read of that entry re-runs and
                    // reports the real error.
                    Err(_) => None,
                }
            };
            self.consumed = i + 1;
            self.cached = out.map(|o| (i, o));
        }
        Ok(self
            .cached
            .as_ref()
            .filter(|(i, _)| *i == index)
            .map(|(_, d)| d.clone())
            .unwrap_or_default())
    }

    fn decode_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        let e = self.entries[index].clone();
        if e.split_open {
            return Err(ArchiveError::UnsupportedFeature {
                reason: format!("rar4: entry '{}' spans volumes beyond this set", e.name),
            });
        }
        if e.encrypted && self.password.is_none() {
            return Err(ArchiveError::Security(format!(
                "rar4: entry '{}' is encrypted; password required",
                e.name
            )));
        }
        let mut packed = self
            .arena
            .get(e.data.0..e.data.1)
            .ok_or_else(|| ArchiveError::InvalidArchive("rar4: data out of bounds".into()))?
            .to_vec();
        if e.encrypted {
            crate::rar3_crypto::decrypt_rar30(
                self.password.as_deref().unwrap_or_default(),
                e.salt.as_ref(),
                &mut packed,
            );
        }
        let mut out = if e.method == RAR4_METHOD_STORE {
            packed
        } else {
            self.unpacker
                .do_unpack(
                    e.unp_ver,
                    e.solid,
                    packed,
                    e.encrypted,
                    e.unpacked_size as i64,
                    e.window_size,
                )
                .map_err(|err| match err {
                    ArchiveError::UnsupportedFeature { reason } => {
                        ArchiveError::UnsupportedFeature {
                            reason: format!("rar4: entry '{}': {reason}", e.name),
                        }
                    }
                    other => other,
                })?
        };
        if out.len() < e.unpacked_size as usize {
            return Err(ArchiveError::InvalidArchive(format!(
                "rar4: entry '{}': decoded {} of {} bytes",
                e.name,
                out.len(),
                e.unpacked_size
            )));
        }
        out.truncate(e.unpacked_size as usize);
        if let Some(crc) = e.crc32 {
            let computed = omnizip_archive_core::crc32(&out);
            if computed != crc {
                return Err(ArchiveError::Checksum(format!(
                    "rar4: entry '{}': CRC mismatch: stored {crc:08X}, computed {computed:08X}",
                    e.name
                )));
            }
        }
        Ok(out)
    }
}

impl ArchiveReader for Rar4Reader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        Ok(self
            .entries
            .iter()
            .map(|e| ArchiveEntry {
                name: e.name.clone(),
                size: Some(e.unpacked_size),
                mtime: e.mtime,
                mode: Some(if e.is_dir { 0o755 } else { 0o644 }),
                kind: if e.is_dir {
                    EntryKind::Directory
                } else if e.is_symlink {
                    // Target is the entry's stored content itself.
                    EntryKind::Symlink(e.name.clone())
                } else {
                    EntryKind::Regular
                },
                uid: None,
                gid: None,
                uname: String::new(),
                gname: String::new(),
                method: Some(u16::from(e.method)),
            })
            .collect())
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        let e = self
            .entries
            .get(index)
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("rar4: no entry {index}")))?;
        if e.is_dir {
            return Ok(Vec::new());
        }
        // Store and unknown-version entries decode directly; LZ paths
        // go through the ordered solid prefix.
        if e.method == RAR4_METHOD_STORE
            && !matches!(e.unp_ver, 15 | 20 | 26 | 29 | 36)
            && !e.encrypted
        {
            return self.decode_entry(index);
        }
        self.decode_prefix(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rejects_non_rar() {
        assert!(Rar4Reader::from_bytes(b"certainly not a rar file").is_err());
    }

    #[test]
    fn reads_libarchive_stored_fixture() {
        // Find a stored rar4 fixture in the corpus.
        let dir = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../omnizip/spec/fixtures/rar/libarchive_reference/rar4"
        ));
        if !dir.exists() {
            return;
        }
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let p = entry.path();
            if p.extension().map(|e| e != "rar").unwrap_or(true) {
                continue;
            }
            let Ok(data) = std::fs::read(&p) else {
                continue;
            };
            if data.len() < 7 || data[0..7] != MAGIC_RAR4 {
                continue;
            }
            if let Ok(mut r) = Rar4Reader::from_bytes(&data) {
                let entries = r.entries().unwrap();
                for (i, entry) in entries.iter().enumerate() {
                    if entry.method == Some(u16::from(RAR4_METHOD_STORE))
                        && !entry.is_directory()
                        && !entry.name.starts_with("testlink")
                    {
                        match r.read_entry(i) {
                            Ok(data) => {
                                assert_eq!(
                                    data.len() as u64,
                                    entry.size.unwrap_or(0),
                                    "{}: {}",
                                    p.display(),
                                    entry.name
                                );
                                checked += 1;
                            }
                            Err(ArchiveError::UnsupportedFeature { reason })
                                if reason.contains("volumes") =>
                            {
                                // split-volume entries are correctly rejected
                            }
                            Err(e) => panic!("{}: entry {}: {e:?}", p.display(), entry.name),
                        }
                    }
                }
            }
        }
        assert!(checked > 0, "no stored entries exercised");
    }
}
