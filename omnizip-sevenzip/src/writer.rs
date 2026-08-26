//! 7z writer — Phase B/C of TODO.containers task 06: non-solid and
//! solid archives (one folder for the whole archive, concatenated
//! unpacked streams with per-file substream sizes and CRCs), folder
//! coders Copy / Deflate / BZip2 / LZMA2, 7zAES-encrypted streams and
//! headers, and multi-volume splits — deterministic by construction:
//! fixed FILETIME mtimes from `WriteOptions`, sorted entry order,
//! stable CRCs, and fixed (zero) AES salt/IV so encrypted output is
//! byte-identical across runs.
#![forbid(unsafe_code)]

use crate::{method, property, SIGNATURE, START_HEADER_SIZE};
use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveError, ArchiveWriter, EntryKind, NewEntry};
use std::collections::BTreeMap;

/// Folder coder selection for the writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SevenZipMethod {
    Copy,
    Deflate,
    Bzip2,
    Lzma2,
}

/// 7zAES parameters for the deterministic writer (no salt, zero IV,
/// 2^19 KDF rounds — the 7-Zip default cycles power).
const AES_CYCLES_POWER: u8 = 19;

impl SevenZipMethod {
    #[must_use]
    pub const fn id(self) -> u64 {
        match self {
            Self::Copy => method::COPY,
            Self::Deflate => method::DEFLATE,
            Self::Bzip2 => method::BZIP2,
            Self::Lzma2 => method::LZMA2,
        }
    }

    fn compress(self, data: &[u8]) -> Result<Vec<u8>, ArchiveError> {
        match self {
            Self::Copy => Ok(data.to_vec()),
            Self::Deflate => omnizip_libdeflate::deflate_dynamic::deflate_dynamic_huffman(data)
                .map(|o| {
                    o.unwrap_or_else(|| {
                        // Dynamic Huffman declined; emit a valid
                        // stored-block deflate stream instead of raw
                        // bytes.
                        omnizip_libdeflate::deflate::deflate_stored(data)
                            .unwrap_or_else(|_| data.to_vec())
                    })
                })
                .map_err(|e| ArchiveError::InvalidArchive(format!("deflate: {e}"))),
            Self::Bzip2 => omnizip_bzip2::compress_framed(data, 9)
                .map_err(|e| ArchiveError::InvalidArchive(format!("bzip2: {e}"))),
            Self::Lzma2 => omnizip_lzma::encoder::lzma2::encode_lzma2_stream(data)
                .map_err(|e| ArchiveError::InvalidArchive(format!("lzma2: {e}"))),
        }
    }

    /// Coder properties (LZMA2 carries the dictionary-size byte).
    #[must_use]
    pub fn properties(self) -> Vec<u8> {
        // 16 MiB window: matches the LZMA2 chunk encoder's cap.
        match self {
            Self::Lzma2 => vec![24],
            _ => Vec::new(),
        }
    }
}

/// Builds a deterministic 7z archive in memory — Phase C adds solid
/// folders, AES encryption of streams and headers, and volume splits.
pub struct SevenZipWriter {
    method: SevenZipMethod,
    solid: bool,
    password: Option<String>,
    /// name → (entry, data), sorted on finish.
    files: BTreeMap<String, (NewEntry, Vec<u8>)>,
    dirs: BTreeMap<String, NewEntry>,
}

impl SevenZipWriter {
    #[must_use]
    pub fn new(method: SevenZipMethod) -> Self {
        Self {
            method,
            solid: false,
            password: None,
            files: BTreeMap::new(),
            dirs: BTreeMap::new(),
        }
    }

    /// One folder for every streamed file (solid archive).
    #[must_use]
    pub const fn with_solid(mut self, solid: bool) -> Self {
        self.solid = solid;
        self
    }

    /// Encrypt file streams and the archive header with 7zAES
    /// (AES-256-CBC, SHA-256 counter KDF). The salt and IV are fixed
    /// to zero so output stays deterministic.
    #[must_use]
    pub fn with_password(mut self, password: &str) -> Self {
        self.password = Some(password.to_string());
        self
    }

    /// Serialize the full archive.
    ///
    /// # Errors
    ///
    /// Compression or encryption failures.
    pub fn finish_bytes(&mut self, options: &WriteOptions) -> Result<Vec<u8>, ArchiveError> {
        let (packed, pack_sizes, folders, substreams, plain_names) = self.encode_folders()?;
        let header =
            self.build_header(options, &pack_sizes, &folders, &substreams, &plain_names)?;
        let out = self.wrap_next_header(packed, header)?;
        Ok(out)
    }

    /// Serialize into multi-volume splits of `volume_size` bytes
    /// (`.7z.001/.002/…` geometry: every volume but the last is
    /// exactly `volume_size` bytes). The caller names the parts.
    ///
    /// # Errors
    ///
    /// As [`Self::finish_bytes`], plus a zero `volume_size`.
    pub fn finish_volumes(
        &mut self,
        options: &WriteOptions,
        volume_size: usize,
    ) -> Result<Vec<Vec<u8>>, ArchiveError> {
        if volume_size == 0 {
            return Err(ArchiveError::InvalidArchive(
                "7z: volume size must be nonzero".into(),
            ));
        }
        let bytes = self.finish_bytes(options)?;
        Ok(bytes
            .chunks(volume_size)
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>())
    }

    /// Compress (and optionally encrypt) every streamed file into
    /// folder(s). Returns (packed bytes, pack sizes, per-folder
    /// metadata, substream descriptor, files-info rows).
    #[allow(clippy::type_complexity)]
    fn encode_folders(
        &self,
    ) -> Result<
        (
            Vec<u8>,
            Vec<u64>,
            Vec<FolderSpec>,
            Option<SubstreamsSpec>,
            Vec<(String, bool, bool, u64, u32)>, // (name, is_dir, has_stream, size, mode)
        ),
        ArchiveError,
    > {
        // Files-info rows: dirs first for a stable, reader-friendly
        // order, then files sorted by name.
        let mut names: Vec<(String, bool, bool, u64, u32)> = Vec::new();
        for (name, entry) in &self.dirs {
            names.push((name.clone(), true, false, 0, entry.mode));
        }
        let mut streamed: Vec<(&String, &Vec<u8>)> = Vec::new();
        for (name, (_, data)) in &self.files {
            if data.is_empty() {
                // Empty files carry no stream: empty-stream +
                // empty-file bit vectors cover them.
                names.push((
                    name.clone(),
                    false,
                    false,
                    0,
                    self.files[name.as_str()].0.mode,
                ));
            } else {
                streamed.push((name, data));
            }
        }
        for (name, data) in &streamed {
            let entry = &self.files[name.as_str()].0;
            names.push(((*name).clone(), false, true, data.len() as u64, entry.mode));
        }

        let mut packed: Vec<u8> = Vec::new();
        let mut pack_sizes: Vec<u64> = Vec::new();
        let mut folders: Vec<FolderSpec> = Vec::new();
        let mut sub_sizes: Vec<u64> = Vec::new();
        let mut sub_crcs: Vec<u32> = Vec::new();

        if self.solid {
            if !streamed.is_empty() {
                let mut plain = Vec::new();
                for (_, data) in &streamed {
                    plain.extend_from_slice(data);
                    sub_sizes.push(data.len() as u64);
                    sub_crcs.push(omnizip_archive_core::crc32(data));
                }
                let compressed = self.method.compress(&plain)?;
                let cipher = self.encrypt_stream(&compressed)?;
                pack_sizes.push(cipher.len() as u64);
                packed.extend_from_slice(&cipher);
                folders.push(self.folder_spec(plain.len(), compressed.len()));
            }
        } else {
            for (_, data) in &streamed {
                let compressed = self.method.compress(data)?;
                let cipher = self.encrypt_stream(&compressed)?;
                pack_sizes.push(cipher.len() as u64);
                packed.extend_from_slice(&cipher);
                folders.push(self.folder_spec(data.len(), compressed.len()));
            }
        }

        let substreams = (!sub_sizes.is_empty() && sub_sizes.len() > 1).then_some(SubstreamsSpec {
            sizes: sub_sizes,
            crcs: sub_crcs,
        });
        Ok((packed, pack_sizes, folders, substreams, names))
    }

    /// Folder metadata for one folder decoding `plain_len` bytes from
    /// `compressed_len` bytes (equal when uncompressed; the AES out
    /// size is the compressed size when encrypting).
    fn folder_spec(&self, plain_len: usize, compressed_len: usize) -> FolderSpec {
        let method_coder = (self.method.id(), self.method.properties());
        if self.password.is_some() {
            FolderSpec {
                coders: vec![
                    (method::AES, aes_coder_properties(AES_CYCLES_POWER)),
                    method_coder,
                ],
                unpack_sizes: vec![compressed_len as u64, plain_len as u64],
                folder_crc: None,
            }
        } else {
            FolderSpec {
                coders: vec![method_coder],
                unpack_sizes: vec![plain_len as u64],
                folder_crc: None,
            }
        }
    }

    /// PKCS#7-pad to the AES block size and encrypt with the
    /// deterministic key/IV derived from the password.
    fn encrypt_stream(&self, data: &[u8]) -> Result<Vec<u8>, ArchiveError> {
        let Some(password) = &self.password else {
            return Ok(data.to_vec());
        };
        let pad = 16 - data.len() % 16;
        let mut buf = data.to_vec();
        buf.extend(std::iter::repeat(pad as u8).take(pad));
        let key = crate::aes256_kdf(password, &[], AES_CYCLES_POWER);
        omnizip_crypto::AesCbc256::new(&key, &[0u8; 16]).encrypt(&mut buf);
        Ok(buf)
    }

    /// Emit the next header: plain, or (with a password) as a
    /// kEncodedHeader folder whose packed stream follows the main
    /// streams — mirroring the geometry 7zz writes.
    fn wrap_next_header(&self, packed: Vec<u8>, header: Vec<u8>) -> Result<Vec<u8>, ArchiveError> {
        let Some(password) = &self.password else {
            let mut out = Vec::with_capacity(START_HEADER_SIZE + packed.len() + header.len());
            out.extend_from_slice(&SIGNATURE);
            out.push(0); // major
            out.push(4); // minor
            let next_offset = packed.len() as u64;
            let next_size = header.len() as u64;
            let mut next_block = Vec::with_capacity(20);
            next_block.extend_from_slice(&next_offset.to_le_bytes());
            next_block.extend_from_slice(&next_size.to_le_bytes());
            next_block.extend_from_slice(&omnizip_archive_core::crc32(&header).to_le_bytes());
            out.extend_from_slice(&omnizip_archive_core::crc32(&next_block).to_le_bytes());
            out.extend_from_slice(&next_block);
            out.extend_from_slice(&packed);
            out.extend_from_slice(&header);
            return Ok(out);
        };

        // Encoded header: compress, pad, encrypt. Copy archives keep
        // the header uncompressed inside the AES folder.
        let compressed = if self.method == SevenZipMethod::Copy {
            header.clone()
        } else {
            self.method.compress(&header)?
        };
        let cipher = {
            let pad = 16 - compressed.len() % 16;
            let mut buf = compressed.clone();
            buf.extend(std::iter::repeat(pad as u8).take(pad));
            let key = crate::aes256_kdf(password, &[], AES_CYCLES_POWER);
            omnizip_crypto::AesCbc256::new(&key, &[0u8; 16]).encrypt(&mut buf);
            buf
        };

        // kEncodedHeader + streams info pointing at the cipher, which
        // sits right after the main packed streams.
        let mut blob = Vec::new();
        blob.push(property::ENCODED_HEADER as u8);
        blob.push(property::PACK_INFO as u8);
        write_number(&mut blob, packed.len() as u64); // pack pos
        write_number(&mut blob, 1); // one stream
        blob.push(property::SIZE as u8);
        write_number(&mut blob, cipher.len() as u64);
        blob.push(property::END as u8);
        blob.push(property::UNPACK_INFO as u8);
        blob.push(property::FOLDER as u8);
        write_number(&mut blob, 1); // one folder
        blob.push(0); // external
        let method_coder = (self.method.id(), self.method.properties());
        let coders: Vec<(u64, Vec<u8>)> = if self.method == SevenZipMethod::Copy {
            vec![(method::AES, aes_coder_properties(AES_CYCLES_POWER))]
        } else {
            vec![
                (method::AES, aes_coder_properties(AES_CYCLES_POWER)),
                method_coder,
            ]
        };
        write_number(&mut blob, coders.len() as u64);
        for (id, props) in &coders {
            let id_bytes = method_id_bytes(*id);
            let flags = id_bytes.len() as u8 | (u8::from(!props.is_empty()) * 0x20);
            blob.push(flags);
            blob.extend_from_slice(&id_bytes);
            if !props.is_empty() {
                write_number(&mut blob, props.len() as u64);
                blob.extend_from_slice(props);
            }
        }
        if coders.len() == 2 {
            // Bind the method's input to the AES output.
            write_number(&mut blob, 1);
            write_number(&mut blob, 0);
        }
        blob.push(property::CODERS_UNPACK_SIZE as u8);
        if coders.len() == 2 {
            write_number(&mut blob, compressed.len() as u64);
        }
        write_number(&mut blob, header.len() as u64);
        blob.push(property::CRC as u8);
        blob.push(1); // all defined
        blob.extend_from_slice(&omnizip_archive_core::crc32(&header).to_le_bytes());
        blob.push(property::END as u8); // end unpack info
        blob.push(property::END as u8); // end streams info

        let mut out =
            Vec::with_capacity(START_HEADER_SIZE + packed.len() + cipher.len() + blob.len());
        out.extend_from_slice(&SIGNATURE);
        out.push(0);
        out.push(4);
        let next_offset = (packed.len() + cipher.len()) as u64;
        let next_size = blob.len() as u64;
        let mut next_block = Vec::with_capacity(20);
        next_block.extend_from_slice(&next_offset.to_le_bytes());
        next_block.extend_from_slice(&next_size.to_le_bytes());
        next_block.extend_from_slice(&omnizip_archive_core::crc32(&blob).to_le_bytes());
        out.extend_from_slice(&omnizip_archive_core::crc32(&next_block).to_le_bytes());
        out.extend_from_slice(&next_block);
        out.extend_from_slice(&packed);
        out.extend_from_slice(&cipher);
        out.extend_from_slice(&blob);
        Ok(out)
    }

    fn build_header(
        &self,
        options: &WriteOptions,
        pack_sizes: &[u64],
        folders: &[FolderSpec],
        substreams: &Option<SubstreamsSpec>,
        names: &[(String, bool, bool, u64, u32)],
    ) -> Result<Vec<u8>, ArchiveError> {
        let mut h = Vec::new();
        h.push(property::HEADER as u8);

        // --- Main streams info -------------------------------------
        if !pack_sizes.is_empty() {
            h.push(property::MAIN_STREAMS_INFO as u8);
            h.push(property::PACK_INFO as u8);
            write_number(&mut h, 0); // pack pos
            write_number(&mut h, pack_sizes.len() as u64);
            h.push(property::SIZE as u8);
            for s in pack_sizes {
                write_number(&mut h, *s);
            }
            h.push(property::END as u8);

            h.push(property::UNPACK_INFO as u8);
            h.push(property::FOLDER as u8);
            write_number(&mut h, folders.len() as u64);
            h.push(0); // external
            for folder in folders {
                write_number(&mut h, folder.coders.len() as u64);
                for (id, props) in &folder.coders {
                    let id_bytes = method_id_bytes(*id);
                    let flags = id_bytes.len() as u8 | (u8::from(!props.is_empty()) * 0x20);
                    h.push(flags);
                    h.extend_from_slice(&id_bytes);
                    if !props.is_empty() {
                        write_number(&mut h, props.len() as u64);
                        h.extend_from_slice(props);
                    }
                }
                if folder.coders.len() == 2 {
                    // Bind pair: in 1 ← out 0 (the pipeline order).
                    write_number(&mut h, 1);
                    write_number(&mut h, 0);
                }
            }
            h.push(property::CODERS_UNPACK_SIZE as u8);
            for folder in folders {
                for s in &folder.unpack_sizes {
                    write_number(&mut h, *s);
                }
            }
            // One stream per folder without substreams info: folder
            // digests double as the per-file CRCs.
            let folder_digests = substreams.is_none();
            if folder_digests {
                h.push(property::CRC as u8);
                h.push(1);
                for (name, _, _, _, _) in names.iter().filter(|n| n.2) {
                    let data = &self.files[name.as_str()].1;
                    h.extend_from_slice(&omnizip_archive_core::crc32(data).to_le_bytes());
                }
            }
            h.push(property::END as u8); // end unpack info
            if let Some(spec) = substreams {
                // Solid: per-file sizes and CRCs at the streams-info
                // level (after unpack info ends).
                h.push(property::SUBSTREAMS_INFO as u8);
                h.push(property::NUM_UNPACK_STREAM as u8);
                write_number(&mut h, spec.sizes.len() as u64);
                h.push(property::SIZE as u8);
                for s in &spec.sizes[..spec.sizes.len() - 1] {
                    write_number(&mut h, *s);
                }
                h.push(property::CRC as u8);
                h.push(1); // all defined
                for crc in &spec.crcs {
                    h.extend_from_slice(&crc.to_le_bytes());
                }
                h.push(property::END as u8);
            }
            h.push(property::END as u8); // end main streams info
        }

        // --- Files info ---------------------------------------------
        h.push(property::FILES_INFO as u8);
        write_number(&mut h, names.len() as u64);

        // Empty-stream vector (dirs + empty files).
        let has_empty = names.iter().any(|n| !n.2);
        if has_empty {
            h.push(property::EMPTY_STREAM as u8);
            let bits: Vec<bool> = names.iter().map(|n| !n.2).collect();
            write_number(&mut h, bits.len().div_ceil(8) as u64);
            write_bit_vector(&mut h, &bits);
            // Empty-file vector among the no-stream entries: set bit
            // = empty file, clear bit = directory.
            let empty_bits: Vec<bool> = names.iter().filter(|n| !n.2).map(|n| !n.1).collect();
            if empty_bits.iter().any(|&b| b) {
                h.push(property::EMPTY_FILE as u8);
                write_number(&mut h, empty_bits.len().div_ceil(8) as u64);
                write_bit_vector(&mut h, &empty_bits);
            }
        }

        // mtime for every entry (all-defined marker first, per
        // ReadBoolVector2).
        h.push(property::MTIME as u8);
        let size = 2 + names.len() * 8;
        write_number(&mut h, size as u64);
        h.push(1); // all defined
        h.push(0); // external
        for _ in names {
            h.extend_from_slice(&crate::unix_to_filetime(options.mtime).to_le_bytes());
        }

        // Windows attributes: unix mode in the high 16 bits (the
        // 7-Zip-on-unix convention) + FILE_ATTRIBUTE_DIRECTORY.
        h.push(property::WIN_ATTRIB as u8);
        let size = 2 + names.len() * 4;
        write_number(&mut h, size as u64);
        h.push(1); // all defined
        h.push(0); // external
        for (_, is_dir, _, _, mode) in names {
            let attr = ((mode & 0o7777) << 16) | if *is_dir { 0x10 } else { 0x80 };
            h.extend_from_slice(&attr.to_le_bytes());
        }

        // Names (UTF-16LE, NUL-terminated).
        h.push(property::NAME as u8);
        let mut name_blob = Vec::new();
        for (name, _, _, _, _) in names {
            for unit in name.encode_utf16() {
                name_blob.extend_from_slice(&unit.to_le_bytes());
            }
            name_blob.extend_from_slice(&0u16.to_le_bytes());
        }
        write_number(&mut h, (name_blob.len() + 1) as u64);
        h.push(0); // external
        h.extend_from_slice(&name_blob);

        h.push(property::END as u8); // end files info
        h.push(property::END as u8); // end header
        Ok(h)
    }
}

/// Per-folder coder chain the writer emits.
struct FolderSpec {
    /// (method id, properties) in decode order: [AES?] then method.
    coders: Vec<(u64, Vec<u8>)>,
    /// One size per coder output stream.
    unpack_sizes: Vec<u64>,
    #[allow(dead_code)]
    folder_crc: Option<u32>,
}

struct SubstreamsSpec {
    sizes: Vec<u64>,
    crcs: Vec<u32>,
}

/// 7zAES coder properties for a salt-less, 16-byte-IV folder coder
/// (the layout `SetDecoderProperties2` parses).
fn aes_coder_properties(cycles_power: u8) -> Vec<u8> {
    let mut props = vec![cycles_power | 0x40, 0x0F];
    props.extend_from_slice(&[0u8; 16]);
    props
}

/// 7-Zip VLI number encoding (canonical: the smallest extra-byte
/// count whose data bits fit).
fn write_number(out: &mut Vec<u8>, value: u64) {
    if value < 0x80 {
        out.push(value as u8);
        return;
    }
    for k in 1..8u32 {
        let data = value >> (8 * k);
        if data != 0 && data < (1u64 << (8 - k)) {
            let mut first: u8 = 0;
            for i in 0..k {
                first |= 1 << (7 - i);
            }
            out.push(first | data as u8);
            for i in 0..k {
                out.push(((value >> (8 * i)) & 0xFF) as u8);
            }
            return;
        }
    }
    out.push(0xFF);
    for i in 0..8 {
        out.push(((value >> (8 * i)) & 0xFF) as u8);
    }
}

/// Minimal big-endian method-id bytes (leading zero bytes stripped).
fn method_id_bytes(id: u64) -> Vec<u8> {
    let mut v = id.to_be_bytes().to_vec();
    while v.first() == Some(&0) && v.len() > 1 {
        v.remove(0);
    }
    v
}

fn write_bit_vector(out: &mut Vec<u8>, bits: &[bool]) {
    let mut packed = vec![0u8; bits.len().div_ceil(8)];
    for (i, &b) in bits.iter().enumerate() {
        if b {
            packed[i / 8] |= 1 << (7 - (i % 8));
        }
    }
    out.extend_from_slice(&packed);
}

impl ArchiveWriter for SevenZipWriter {
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
        entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        // 7z symlinks: store the target as the file body with the
        // unix symlink mode in attributes.
        let target = match &entry.kind {
            EntryKind::Symlink(t) => t.clone(),
            _ => {
                return Err(ArchiveError::InvalidArchive(
                    "add_symlink expects a Symlink entry".into(),
                ));
            }
        };
        self.files
            .insert(entry.name.clone(), (entry.clone(), target.into_bytes()));
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ArchiveError> {
        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::SevenZipReader;
    use omnizip_archive_core::ArchiveReader;

    fn opts() -> WriteOptions {
        WriteOptions::deterministic().with_mtime(1_700_000_000)
    }

    fn build(m: SevenZipMethod, solid: bool) -> Vec<u8> {
        let mut w = SevenZipWriter::new(m).with_solid(solid);
        w.add_directory(&NewEntry::directory("doc", &opts()), &opts())
            .unwrap();
        w.add_file(
            &NewEntry::file("doc/readme.txt", &opts()),
            b"seven zip round trip\n".repeat(20).as_slice(),
            &opts(),
        )
        .unwrap();
        w.add_file(
            &NewEntry::file("doc/data.bin", &opts()),
            &[0x77; 1024],
            &opts(),
        )
        .unwrap();
        w.finish_bytes(&opts()).unwrap()
    }

    #[test]
    fn round_trips_all_methods() {
        for m in [
            SevenZipMethod::Copy,
            SevenZipMethod::Deflate,
            SevenZipMethod::Bzip2,
            SevenZipMethod::Lzma2,
        ] {
            for solid in [false, true] {
                let bytes = build(m, solid);
                let mut r = SevenZipReader::from_bytes(&bytes).unwrap();
                let entries = r.entries().unwrap();
                let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
                assert_eq!(
                    names,
                    vec!["doc", "doc/data.bin", "doc/readme.txt"],
                    "{m:?} solid={solid}"
                );
                let readme = names.iter().position(|n| *n == "doc/readme.txt").unwrap();
                assert_eq!(
                    r.read_entry(readme).unwrap(),
                    b"seven zip round trip\n".repeat(20),
                    "{m:?} solid={solid}"
                );
                let bin = names.iter().position(|n| *n == "doc/data.bin").unwrap();
                assert_eq!(
                    r.read_entry(bin).unwrap(),
                    vec![0x77; 1024],
                    "{m:?} solid={solid}"
                );
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            build(SevenZipMethod::Deflate, true),
            build(SevenZipMethod::Deflate, true)
        );
    }

    #[test]
    fn solid_has_one_folder_and_substreams() {
        let bytes = build(SevenZipMethod::Copy, true);
        let r = SevenZipReader::from_bytes(&bytes).unwrap();
        assert_eq!(r.stream_info.folders.len(), 1);
        assert_eq!(r.stream_info.num_unpack_streams_in_folders, vec![2]);
        assert_eq!(r.stream_info.unpack_sizes, vec![1024, 420]);
        assert!(r.stream_info.digests.iter().all(|d| d.is_some()));
    }

    #[test]
    fn non_solid_is_one_folder_per_file() {
        let bytes = build(SevenZipMethod::Copy, false);
        let r = SevenZipReader::from_bytes(&bytes).unwrap();
        assert_eq!(r.stream_info.folders.len(), 2);
    }

    #[test]
    fn empty_file_is_a_file_not_a_dir() {
        let mut w = SevenZipWriter::new(SevenZipMethod::Copy).with_solid(true);
        w.add_directory(&NewEntry::directory("d", &opts()), &opts())
            .unwrap();
        w.add_file(&NewEntry::file("d/empty.txt", &opts()), b"", &opts())
            .unwrap();
        w.add_file(&NewEntry::file("d/full.txt", &opts()), b"x", &opts())
            .unwrap();
        let bytes = w.finish_bytes(&opts()).unwrap();
        let mut r = SevenZipReader::from_bytes(&bytes).unwrap();
        let entries = r.entries().unwrap();
        let empty = entries.iter().find(|e| e.name == "d/empty.txt").unwrap();
        assert_eq!(empty.kind, EntryKind::Regular);
        assert_eq!(empty.size, Some(0));
        assert_eq!(r.read_entry(1).unwrap(), b""); // between dir and full.txt
    }

    #[test]
    fn encrypted_headers_round_trip() {
        for solid in [false, true] {
            for m in [
                SevenZipMethod::Copy,
                SevenZipMethod::Deflate,
                SevenZipMethod::Lzma2,
            ] {
                let mut w = SevenZipWriter::new(m)
                    .with_solid(solid)
                    .with_password("secret");
                w.add_file(
                    &NewEntry::file("a.txt", &opts()),
                    b"hidden contents\n",
                    &opts(),
                )
                .unwrap();
                w.add_file(&NewEntry::file("b.bin", &opts()), &[0x5A; 300], &opts())
                    .unwrap();
                let bytes = w.finish_bytes(&opts()).unwrap();

                // The header is encoded (0x17 first byte of the next header).
                let start = u64::from_le_bytes(bytes[12..20].try_into().unwrap()) as usize;
                assert_eq!(bytes[32 + start], 0x17, "{m:?} solid={solid}");

                let mut r =
                    SevenZipReader::from_bytes_with_password(&bytes, Some("secret")).unwrap();
                let entries = r.entries().unwrap();
                assert_eq!(entries.len(), 2, "{m:?} solid={solid}");
                assert_eq!(r.read_entry(0).unwrap(), b"hidden contents\n");
                assert_eq!(r.read_entry(1).unwrap(), vec![0x5A; 300]);

                assert!(
                    SevenZipReader::from_bytes_with_password(&bytes, Some("wrong")).is_err(),
                    "{m:?} solid={solid}"
                );
            }
        }
    }

    #[test]
    fn encrypted_archive_is_deterministic() {
        let build_pw = || {
            let mut w = SevenZipWriter::new(SevenZipMethod::Lzma2)
                .with_solid(true)
                .with_password("secret");
            w.add_file(
                &NewEntry::file("a.txt", &opts()),
                b"deterministic\n",
                &opts(),
            )
            .unwrap();
            w.finish_bytes(&opts()).unwrap()
        };
        assert_eq!(build_pw(), build_pw());
    }

    #[test]
    fn volumes_split_and_reassemble() {
        let mut w = SevenZipWriter::new(SevenZipMethod::Copy).with_solid(true);
        w.add_file(&NewEntry::file("a.txt", &opts()), &[0x11; 5000], &opts())
            .unwrap();
        let vols = w.finish_volumes(&opts(), 1024).unwrap();
        assert_eq!(vols.len(), 5); // 5098-byte archive -> 4 full + 1 remainder
        for (i, v) in vols.iter().enumerate() {
            if i + 1 < vols.len() {
                assert_eq!(v.len(), 1024);
            }
        }
        let joined: Vec<u8> = vols.concat();
        let mut r = SevenZipReader::from_bytes(&joined).unwrap();
        assert_eq!(r.read_entry(0).unwrap(), vec![0x11; 5000]);

        let single = {
            let mut w = SevenZipWriter::new(SevenZipMethod::Copy).with_solid(true);
            w.add_file(&NewEntry::file("a.txt", &opts()), &[0x11; 5000], &opts())
                .unwrap();
            w.finish_bytes(&opts()).unwrap()
        };
        assert_eq!(joined, single);
    }

    #[test]
    fn vli_round_trip() {
        use crate::parser::HeaderParser;
        for value in [
            0u64,
            1,
            0x7F,
            0x80,
            0xFF,
            0x100,
            0x1234,
            0xFFFF,
            0x1_0000,
            u64::from(u32::MAX),
            0x00FF_FFFF_FFFF_FFFF,
        ] {
            let mut buf = Vec::new();
            write_number(&mut buf, value);
            let mut p = HeaderParser::new(&buf);
            assert_eq!(
                p.number().unwrap(),
                value,
                "value {value:x} -> {:02x?}",
                buf
            );
        }
    }
}
