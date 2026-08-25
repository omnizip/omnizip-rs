//! RAR5 reader/writer — VINT block headers per the RAR5 spec:
//! each block is `crc32(header-after-crc)` ‖ `vint header_size` ‖
//! `vint type` ‖ `vint flags` ‖ [`vint extra_size`] ‖ [`vint data_size`]
//! ‖ header-area content ‖ data area. File headers carry the name and
//! compression info; STORE data areas extract directly with CRC32
//! verification. The writer emits deterministic STORE archives.
#![forbid(unsafe_code)]

use crate::{
    host_os, rar5_block, rar5_decode_comp_info, rar5_file_flags, rar5_header_flags, write_vint,
    MAGIC_RAR5,
};
use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{
    ArchiveEntry, ArchiveError, ArchiveReader, ArchiveWriter, EntryKind, NewEntry,
};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone)]
struct RawEntry {
    name: String,
    is_dir: bool,
    window_size: usize,
    unpacked_size: u64,
    crc32: Option<u32>,
    method: u64,
    encrypted: bool,
    mtime: Option<u64>,
    /// Target path for redirection (hardlink/symlink) entries.
    symlink_target: Option<String>,
    /// BLAKE2sp-256 from the EX_HASH extra record, when present.
    blake2sp: Option<[u8; 32]>,
    /// Entry assembled from multiple volume parts; per-part CRC32
    /// fields are not the whole-content CRC (the reference reader
    /// skips verification for these).
    split: bool,
    /// Byte range of the packed data in the file.
    data: (usize, usize),
}

/// Reads a RAR5 archive held in memory.
pub struct Rar5Reader {
    /// Contiguous packed data for every entry (split parts stitched).
    arena: Vec<u8>,
    entries: Vec<RawEntry>,
    /// Solid streams must be decoded in archive order; this tracks how
    /// far the shared window has been advanced.
    main_solid: bool,
    solid: crate::rar5_unpack::SolidState,
    consumed: usize,
    cached: Option<(usize, Vec<u8>)>,
}

fn verify_hashes(
    name: &str,
    crc: Option<u32>,
    blake2sp: Option<&[u8; 32]>,
    split: bool,
    data: &[u8],
) -> Result<(), ArchiveError> {
    if let Some(crc) = crc.filter(|_| !split) {
        let computed = omnizip_archive_core::crc32(data);
        if computed != crc {
            return Err(ArchiveError::Checksum(format!(
                "rar5: entry '{name}': CRC mismatch: stored {crc:08X}, computed {computed:08X}"
            )));
        }
    }
    if let Some(want) = blake2sp {
        let got = omnizip_crypto::blake2sp::blake2sp_256(data);
        if &got != want {
            return Err(ArchiveError::Checksum(format!(
                "rar5: entry '{name}': BLAKE2sp mismatch: stored {}, computed {}",
                want.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                got.iter().map(|b| format!("{b:02x}")).collect::<String>()
            )));
        }
    }
    Ok(())
}

impl Rar5Reader {
    /// Parse a RAR5 archive.
    ///
    /// # Errors
    ///
    /// [`ArchiveError`] on signature/header problems.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        if data.len() < 8 || data[0..8] != MAGIC_RAR5 {
            return Err(ArchiveError::InvalidArchive(
                "rar5: invalid signature".into(),
            ));
        }
        let mut entries: Vec<RawEntry> = Vec::new();
        let mut arena: Vec<u8> = Vec::new();
        let mut pos = 8usize;
        let mut seen_main = false;
        let mut main_solid = false;
        let mut continuation: Option<usize> = None;

        loop {
            let Some(block) = parse_block(data, &mut pos)? else {
                break;
            };
            match block.kind {
                rar5_block::MAIN => {
                    let mut c = block.content;
                    if let Ok((flags, _)) = read_vint(data, &mut c) {
                        main_solid = flags & 0x0004 != 0;
                    }
                    seen_main = true;
                }
                rar5_block::END => {
                    // A next volume's signature may sit after some
                    // padding bytes; scan for it like the reference.
                    if let Some(at) = find_signature(data, block.end) {
                        pos = at + MAGIC_RAR5.len();
                        continue;
                    }
                    break;
                }
                rar5_block::FILE => {
                    let mut entry = parse_file_header(data, &block)?;
                    if let Some(e) = &mut entry {
                        e.split = block.flags
                            & (rar5_header_flags::SPLIT_BEFORE | rar5_header_flags::SPLIT_AFTER)
                            != 0;
                        // Packed slices from different volumes are not
                        // contiguous (headers interleave), so every
                        // slice is copied into a per-reader arena and
                        // split parts append to their pending entry.
                        let slice = data.get(e.data.0..e.data.1).ok_or_else(|| {
                            ArchiveError::InvalidArchive("rar5: data out of bounds".into())
                        })?;
                        let split_before = block.flags & rar5_header_flags::SPLIT_BEFORE != 0;
                        let split_after = block.flags & rar5_header_flags::SPLIT_AFTER != 0;
                        match (split_before, continuation) {
                            (true, Some(idx)) => {
                                arena.extend_from_slice(slice);
                                // Keep the first slice's start; only the
                                // end advances as parts accumulate.
                                entries[idx].data.1 = arena.len();
                                // Continuation headers repeat the first
                                // header's full unpacked size and
                                // compression info; only the packed
                                // bytes accumulate.
                                continuation = if split_after { Some(idx) } else { None };
                            }
                            (true, None) => {
                                return Err(ArchiveError::InvalidArchive(
                                    "rar5: split continuation without an open entry".into(),
                                ));
                            }
                            _ => {
                                let start = arena.len();
                                arena.extend_from_slice(slice);
                                e.data = (start, arena.len());
                                continuation = if split_after && !e.is_dir {
                                    Some(entries.len())
                                } else {
                                    None
                                };
                                entries.push(e.clone());
                            }
                        }
                    }
                }
                _ => {}
            }
            pos = block.end;
        }
        if !seen_main {
            return Err(ArchiveError::InvalidArchive(
                "rar5: missing main archive header".into(),
            ));
        }
        Ok(Self {
            arena,
            entries,
            main_solid,
            solid: crate::rar5_unpack::SolidState::default(),
            consumed: 0,
            cached: None,
        })
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

    /// Open a multi-volume set: the parts are concatenated in order
    /// and parsed as one stream; each part's repeated signature and
    /// MAIN header are skipped on the fly. Split entries are stitched
    /// across parts by the SPLIT_BEFORE/SPLIT_AFTER flags.
    ///
    /// # Errors
    ///
    /// IO or archive errors from any part.
    pub fn open_volumes(paths: &[std::path::PathBuf]) -> Result<Self, ArchiveError> {
        if paths.is_empty() {
            return Err(ArchiveError::InvalidArchive(
                "rar5: no volumes given".into(),
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
    /// parts (`.partNN.rar` numbering, then `.rNN`/`.sNN`).
    ///
    /// # Errors
    ///
    /// IO or archive errors.
    pub fn open_volume_set(first: &Path) -> Result<Self, ArchiveError> {
        let mut parts = vec![first.to_path_buf()];
        let name = first
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Some(idx) = name.find(".part") {
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
        }
        Self::open_volumes(&parts)
    }
}

struct Block {
    kind: u64,
    flags: u64,
    /// Offset just past the header-size vint and type/flags/size
    /// fields (start of header-area content).
    content: usize,
    /// Header-area end == data-area start (or block end when no data).
    header_end: usize,
    /// End of the whole block (data included).
    end: usize,
    /// Size of the extra area (last `extra_size` bytes of the header
    /// area, 0 when the EXTRA_AREA flag is clear).
    extra_size: usize,
}

fn parse_block(data: &[u8], pos: &mut usize) -> Result<Option<Block>, ArchiveError> {
    let start = *pos;
    let rest = data.get(start..).ok_or(archive_end())?;
    if rest.len() < 4 {
        return Ok(None);
    }
    let crc = u32::from_le_bytes(rest[0..4].try_into().expect("4"));
    let mut p = start + 4;
    let (header_size, _) = read_vint(data, &mut p)?;
    if header_size == 0 || header_size > (1 << 30) {
        return Err(ArchiveError::InvalidArchive(
            "rar5: implausible header size".into(),
        ));
    }
    let (kind, _) = read_vint(data, &mut p)?;
    let (flags, _) = read_vint(data, &mut p)?;
    let mut extra_size = 0u64;
    if flags & rar5_header_flags::EXTRA_AREA != 0 {
        extra_size = read_vint(data, &mut p)?.0;
    }
    let mut data_size = 0u64;
    if flags & rar5_header_flags::DATA_AREA != 0 {
        data_size = read_vint(data, &mut p)?.0;
    }
    let content = p;
    // header_size counts type..content end (after the size vint).
    let header_end = start + 4 + vint_header_size(header_size) + header_size as usize;
    let end = header_end + data_size as usize;
    if end > data.len() || extra_size as usize > header_end.saturating_sub(content) {
        return Err(ArchiveError::InvalidArchive(
            "rar5: block out of bounds".into(),
        ));
    }
    // CRC32 covers the header bytes after the CRC field.
    let computed = omnizip_archive_core::crc32(&data[start + 4..header_end]);
    if computed != crc {
        return Err(ArchiveError::Checksum(format!(
            "rar5: header CRC mismatch at {start}: stored {crc:08X}, computed {computed:08X}"
        )));
    }
    Ok(Some(Block {
        kind,
        flags,
        content,
        header_end,
        end,
        extra_size: extra_size as usize,
    }))
}

/// The header_size vint's own length is not included in header_size;
/// recompute it from the value.
fn vint_header_size(value: u64) -> usize {
    crate::vint_len(value)
}

fn find_signature(data: &[u8], from: usize) -> Option<usize> {
    (from..data.len().saturating_sub(MAGIC_RAR5.len()))
        .find(|&i| data.get(i..i + MAGIC_RAR5.len()) == Some(&MAGIC_RAR5[..]))
}

fn archive_end() -> ArchiveError {
    ArchiveError::InvalidArchive("rar5: unexpected end of archive".into())
}

fn read_vint(data: &[u8], pos: &mut usize) -> Result<(u64, usize), ArchiveError> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    let start = *pos;
    loop {
        let byte = *data.get(*pos).ok_or_else(archive_end)?;
        *pos += 1;
        result |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 63 {
            return Err(ArchiveError::InvalidArchive("rar5: vint too long".into()));
        }
    }
    Ok((result, *pos - start))
}

fn parse_file_header(data: &[u8], block: &Block) -> Result<Option<RawEntry>, ArchiveError> {
    let mut p = block.content;
    let (file_flags, _) = read_vint(data, &mut p)?;
    let (unpacked_size, _) = read_vint(data, &mut p)?;
    let (_attributes, _) = read_vint(data, &mut p)?;
    let mut mtime = None;
    if file_flags & rar5_file_flags::TIME_PRESENT != 0 {
        let raw = u32::from_le_bytes(
            data.get(p..p + 4)
                .ok_or_else(archive_end)?
                .try_into()
                .map_err(|_| archive_end())?,
        );
        p += 4;
        mtime = Some(u64::from(raw));
    }
    let mut crc = None;
    if file_flags & rar5_file_flags::CRC32_PRESENT != 0 {
        crc = Some(u32::from_le_bytes(
            data.get(p..p + 4)
                .ok_or_else(archive_end)?
                .try_into()
                .map_err(|_| archive_end())?,
        ));
        p += 4;
    }
    let (comp_info, _) = read_vint(data, &mut p)?;
    let (_host_os, _) = read_vint(data, &mut p)?;
    let (name_len, _) = read_vint(data, &mut p)?;
    let name_len = name_len as usize;
    let name_bytes = data.get(p..p + name_len).ok_or_else(archive_end)?;
    let name = String::from_utf8_lossy(name_bytes).into_owned();

    let (_, _, method, _) = rar5_decode_comp_info(comp_info);
    let is_dir = file_flags & rar5_file_flags::IS_DIR != 0;
    let extra = scan_extra(data, block, p + name_len);

    Ok(Some(RawEntry {
        name,
        is_dir,
        window_size: crate::rar5_unpack::window_size_from_comp_info(comp_info),
        unpacked_size,
        crc32: crc,
        method,
        encrypted: extra.encrypted,
        mtime,
        symlink_target: extra.symlink_target,
        blake2sp: extra.blake2sp,
        split: false,
        data: (block.header_end, block.end),
    }))
}

/// Extra-area record types.
mod extra_record {
    pub const CRYPT: u64 = 0x01;
    pub const HASH: u64 = 0x02;
    pub const REDIR: u64 = 0x05;
}

/// Parsed extra-area facts for one file header.
struct ExtraFacts {
    symlink_target: Option<String>,
    encrypted: bool,
    blake2sp: Option<[u8; 32]>,
}

/// Walk the extra area (records are `size vint` covering `type vint +
/// data`) for the CRYPT, REDIR, and HASH records. `name_end` is where
/// the regular header fields end.
fn scan_extra(data: &[u8], block: &Block, name_end: usize) -> ExtraFacts {
    let mut out = ExtraFacts {
        symlink_target: None,
        encrypted: false,
        blake2sp: None,
    };
    if block.extra_size == 0 || name_end > block.header_end {
        return out;
    }
    let mut p = block.header_end - block.extra_size;
    let end = block.header_end;
    while p < end {
        let Ok((size, _)) = read_vint(data, &mut p) else {
            return out;
        };
        let rec_end = p + size as usize;
        if rec_end > end {
            return out;
        }
        let Ok((kind, _)) = read_vint(data, &mut p) else {
            return out;
        };
        match kind {
            extra_record::CRYPT => out.encrypted = true,
            extra_record::HASH => {
                // hash-type vint (0 = BLAKE2sp) followed by 32 bytes.
                if let Ok((hash_type, _)) = read_vint(data, &mut p) {
                    if hash_type == 0 {
                        if let Some(bytes) = data.get(p..p + 32) {
                            out.blake2sp = Some(bytes.try_into().expect("32"));
                        }
                    }
                }
            }
            extra_record::REDIR => {
                // type vint, flags vint, name-length vint, name bytes.
                let redir_type = read_vint(data, &mut p);
                let flags = read_vint(data, &mut p);
                if let (Ok(_), Ok(_)) = (redir_type, flags) {
                    if let Ok((n, _)) = read_vint(data, &mut p) {
                        let n = n as usize;
                        if let Some(b) = data.get(p..p + n) {
                            out.symlink_target = Some(String::from_utf8_lossy(b).into_owned());
                        }
                    }
                }
            }
            _ => {}
        }
        p = rec_end;
    }
    out
}

impl Rar5Reader {
    /// Decode LZ entries in archive order up to `index`, maintaining
    /// the shared solid window; returns entry `index`'s bytes.
    fn decode_solid_prefix(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        if index < self.consumed {
            if let Some((i, ref data)) = self.cached {
                if i == index {
                    return Ok(data.clone());
                }
            }
            // Backwards random access: rebuild the stream from the top.
            self.solid = crate::rar5_unpack::SolidState::default();
            self.consumed = 0;
            self.cached = None;
        }
        while self.consumed <= index {
            let i = self.consumed;
            let e = &self.entries[i];
            let output = if e.is_dir || e.method == 0 || e.symlink_target.is_some() {
                Vec::new()
            } else {
                let packed = self
                    .arena
                    .get(e.data.0..e.data.1)
                    .ok_or_else(|| ArchiveError::InvalidArchive("rar5: data out of bounds".into()))?
                    .to_vec();
                // The bit reader legitimately looks ahead past the last
                // block into whatever follows; keep those real bytes.
                let tail = self
                    .arena
                    .get(e.data.1..(e.data.1 + 8).min(self.arena.len()))
                    .unwrap_or(&[])
                    .to_vec();
                crate::rar5_unpack::set_lookahead_tail(tail);
                let mut state = std::mem::take(&mut self.solid);
                if !self.main_solid {
                    state = crate::rar5_unpack::SolidState::default();
                }
                let res = crate::rar5_unpack::unpack_lz(
                    &packed,
                    e.unpacked_size,
                    e.window_size,
                    &mut state,
                );
                // libarchive's reset_file_context: in solid archives the
                // window persists and the base offset advances by this
                // entry's contribution.
                state.solid_offset += state.last_advance;
                self.solid = state;
                let out = res?;
                verify_hashes(&e.name, e.crc32, e.blake2sp.as_ref(), e.split, &out)?;
                out
            };
            self.consumed = i + 1;
            self.cached = Some((i, output));
        }
        Ok(self
            .cached
            .as_ref()
            .filter(|(i, _)| *i == index)
            .map(|(_, d)| d.clone())
            .unwrap_or_default())
    }
}

impl ArchiveReader for Rar5Reader {
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
                } else if let Some(target) = &e.symlink_target {
                    EntryKind::Symlink(target.clone())
                } else {
                    EntryKind::Regular
                },
                uid: None,
                gid: None,
                uname: String::new(),
                gname: String::new(),
                method: Some(e.method as u16),
            })
            .collect())
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        let entry = self
            .entries
            .get(index)
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("rar5: no entry {index}")))?
            .clone();
        let e = &entry;
        if e.is_dir {
            return Ok(Vec::new());
        }
        if let Some(target) = &e.symlink_target {
            return Ok(target.as_bytes().to_vec());
        }
        if e.encrypted {
            return Err(ArchiveError::Security(format!(
                "rar5: entry '{}' is encrypted; password required",
                e.name
            )));
        }
        if e.method == 0 {
            self.consumed = self.consumed.max(index + 1);
            let raw = self
                .arena
                .get(e.data.0..e.data.1)
                .ok_or_else(|| ArchiveError::InvalidArchive("rar5: data out of bounds".into()))?;
            verify_hashes(&e.name, e.crc32, e.blake2sp.as_ref(), e.split, raw)?;
            return Ok(raw.to_vec());
        }
        let out = self.decode_solid_prefix(index)?;
        verify_hashes(&e.name, e.crc32, e.blake2sp.as_ref(), e.split, &out)?;
        Ok(out)
    }
}

/// Builds deterministic RAR5 STORE archives.
pub struct Rar5Writer {
    files: BTreeMap<String, (NewEntry, Vec<u8>)>,
    dirs: BTreeMap<String, NewEntry>,
}

impl Rar5Writer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            dirs: BTreeMap::new(),
        }
    }

    /// Serialize the archive.
    ///
    /// # Errors
    ///
    /// Never in practice.
    pub fn finish_bytes(&mut self, options: &WriteOptions) -> Result<Vec<u8>, ArchiveError> {
        let mut out = Vec::new();
        out.extend_from_slice(&MAGIC_RAR5);
        write_block(&mut out, rar5_block::MAIN, 0, vec![0], &[]);

        // Directories first (sorted), then files (sorted): files
        // carry their data; directories have empty bodies.
        let dir_names: Vec<String> = self.dirs.keys().cloned().collect();
        for name in &dir_names {
            let body = file_header_body(name, &[], true, options);
            write_block(&mut out, rar5_block::FILE, 0, body, &[]);
        }
        for (name, entry) in &self.files {
            let body = file_header_body(name, &entry.1, false, options);
            write_block(
                &mut out,
                rar5_block::FILE,
                rar5_header_flags::DATA_AREA,
                body,
                &entry.1,
            );
        }

        // END body: end-of-archive flags vint (0 = no next volume);
        // 7-Zip rejects an END block without it.
        write_block(&mut out, rar5_block::END, 0, vec![0], &[]);
        Ok(out)
    }
}

impl Default for Rar5Writer {
    fn default() -> Self {
        Self::new()
    }
}

fn file_header_body(name: &str, data: &[u8], is_dir: bool, options: &WriteOptions) -> Vec<u8> {
    let mut b = Vec::new();
    let mut file_flags = rar5_file_flags::TIME_PRESENT | rar5_file_flags::CRC32_PRESENT;
    if is_dir {
        file_flags |= rar5_file_flags::IS_DIR;
    }
    write_vint(&mut b, file_flags);
    write_vint(&mut b, data.len() as u64);
    // Unix attributes: regular file 0o100644 / dir 0o040755.
    let attr: u64 = if is_dir { 0o040_755 } else { 0o100_644 };
    write_vint(&mut b, attr);

    // mtime: 4-byte Unix seconds (per spec, flag 0x0002).
    b.extend_from_slice(&(options.mtime as u32).to_le_bytes());

    b.extend_from_slice(&omnizip_archive_core::crc32(data).to_le_bytes());
    write_vint(&mut b, crate::rar5_comp_info(0, false)); // method 0 = store
    write_vint(&mut b, host_os::UNIX);
    let name_bytes = name.as_bytes();
    write_vint(&mut b, name_bytes.len() as u64);
    b.extend_from_slice(name_bytes);
    b
}

fn write_block(out: &mut Vec<u8>, kind: u64, flags: u64, body: Vec<u8>, data: &[u8]) {
    // Build the size-then-content stream, then compute header_size
    // over type..end (excluding the size vint itself).
    let mut sized = Vec::new();
    let flags = if !data.is_empty() {
        flags | rar5_header_flags::DATA_AREA
    } else {
        flags
    };
    write_vint(&mut sized, kind);
    write_vint(&mut sized, flags);
    if flags & rar5_header_flags::DATA_AREA != 0 {
        write_vint(&mut sized, data.len() as u64);
    }
    let header_size = sized.len() + body.len();
    let mut full = Vec::new();
    write_vint(&mut full, header_size as u64);
    full.extend_from_slice(&sized);
    full.extend_from_slice(&body);

    let crc = omnizip_archive_core::crc32(&full);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&full);
    out.extend_from_slice(data);
}

impl ArchiveWriter for Rar5Writer {
    fn add_file(
        &mut self,
        entry: &NewEntry,
        data: &[u8],
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.files
            .insert(entry.name.clone(), (entry.clone(), data.to_vec()));
        Ok(())
    }

    fn add_directory(
        &mut self,
        entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.dirs.insert(entry.name.clone(), entry.clone());
        Ok(())
    }

    fn add_symlink(
        &mut self,
        _entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        Err(ArchiveError::UnsupportedFeature {
            reason: "rar5: symlink writing not supported".into(),
        })
    }

    fn finish(&mut self) -> Result<(), ArchiveError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn build() -> Vec<u8> {
        let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
        let mut w = Rar5Writer::new();
        w.add_directory(&NewEntry::directory("docs", &opts), &opts)
            .unwrap();
        w.add_file(
            &NewEntry::file("docs/readme.txt", &opts),
            b"rar5 round trip\n".repeat(40).as_slice(),
            &opts,
        )
        .unwrap();
        w.finish_bytes(&opts).unwrap()
    }

    #[test]
    fn round_trip_store() {
        let bytes = build();
        let mut r = Rar5Reader::from_bytes(&bytes).unwrap();
        let entries = r.entries().unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"docs"), "{names:?}");
        assert!(names.contains(&"docs/readme.txt"), "{names:?}");
        let idx = names.iter().position(|n| *n == "docs/readme.txt").unwrap();
        assert_eq!(r.read_entry(idx).unwrap(), b"rar5 round trip\n".repeat(40));
    }

    #[test]
    fn deterministic() {
        assert_eq!(build(), build());
    }

    #[test]
    fn reads_libarchive_stored_fixture() {
        let fixture = Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../omnizip/spec/fixtures/rar/libarchive_reference/test_read_format_rar5_stored.rar"
        ));
        if !fixture.exists() {
            return;
        }
        let mut r = Rar5Reader::open(fixture).unwrap();
        let entries = r.entries().unwrap();
        let idx = entries
            .iter()
            .position(|e| e.name == "helloworld.txt")
            .expect("fixture entry");
        let data = r.read_entry(idx).unwrap();
        assert_eq!(data, b"hello libarchive test suite!\n");
    }
}
