//! 7z writer — Phase B of TODO.containers task 06: non-solid archives
//! (one folder per file), coder methods Copy / Deflate / BZip2 (the
//! in-house encoders; LZMA2 folders arrive with the codec's encode
//! path), deterministic by construction — fixed FILETIME mtimes from
//! `WriteOptions`, sorted entry order, stable CRCs.
#![forbid(unsafe_code)]

use crate::{method, property, START_HEADER_SIZE, SIGNATURE};
use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveError, ArchiveWriter, EntryKind, NewEntry};
use std::collections::BTreeMap;

/// Folder coder selection for the writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SevenZipMethod {
    Copy,
    Deflate,
    Bzip2,
}

impl SevenZipMethod {
    #[must_use]
    pub const fn id(self) -> u64 {
        match self {
            Self::Copy => method::COPY,
            Self::Deflate => method::DEFLATE,
            Self::Bzip2 => method::BZIP2,
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
        }
    }
}

/// Builds a deterministic non-solid 7z archive in memory.
pub struct SevenZipWriter {
    method: SevenZipMethod,
    /// name → (entry, data), sorted on finish.
    files: BTreeMap<String, (NewEntry, Vec<u8>)>,
    dirs: BTreeMap<String, NewEntry>,
}

impl SevenZipWriter {
    #[must_use]
    pub fn new(method: SevenZipMethod) -> Self {
        Self {
            method,
            files: BTreeMap::new(),
            dirs: BTreeMap::new(),
        }
    }

    /// Serialize the full archive.
    ///
    /// # Errors
    ///
    /// Compression failures.
    pub fn finish_bytes(&mut self, options: &WriteOptions) -> Result<Vec<u8>, ArchiveError> {
        // Compress every file into its own folder; dirs and empty
        // files carry no stream.
        let mut packed: Vec<u8> = Vec::new();
        let mut pack_sizes: Vec<u64> = Vec::new();
        let mut unpack_sizes: Vec<u64> = Vec::new();
        let mut file_order: Vec<&String> = Vec::new();

        for (name, (_, data)) in &self.files {
            let compressed = self.method.compress(data)?;
            pack_sizes.push(compressed.len() as u64);
            unpack_sizes.push(data.len() as u64);
            packed.extend_from_slice(&compressed);
            file_order.push(name);
        }

        // Files-info layout: every entry (dirs first for a stable,
        // reader-friendly order), with empty-stream bits for anything
        // without a stream.
        let mut names: Vec<(String, bool, bool, u64, u32)> = Vec::new(); // (name, is_dir, has_stream, size, mode)
        for (name, entry) in &self.dirs {
            names.push((name.clone(), true, false, 0, entry.mode));
        }
        for name in &file_order {
            let (entry, data) = &self.files[name.as_str()];
            names.push(((*name).clone(), false, true, data.len() as u64, entry.mode));
        }

        let header = self.build_header(options, &pack_sizes, &unpack_sizes, &names)?;

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
        Ok(out)
    }

    fn build_header(
        &self,
        options: &WriteOptions,
        pack_sizes: &[u64],
        unpack_sizes: &[u64],
        names: &[(String, bool, bool, u64, u32)],
    ) -> Result<Vec<u8>, ArchiveError> {
        let mut h = Vec::new();
        h.push(property::HEADER as u8);

        // --- Main streams info -------------------------------------
        if !pack_sizes.is_empty() {
            h.push(property::MAIN_STREAMS_INFO as u8);
            // Pack info: position 0, N streams, sizes.
            h.push(property::PACK_INFO as u8);
            write_number(&mut h, 0); // pack pos
            write_number(&mut h, pack_sizes.len() as u64);
            h.push(property::SIZE as u8);
            for s in pack_sizes {
                write_number(&mut h, *s);
            }
            h.push(property::END as u8);

            // Unpack info: N folders of one coder each, unpack sizes.
            h.push(property::UNPACK_INFO as u8);
            h.push(property::FOLDER as u8);
            write_number(&mut h, pack_sizes.len() as u64);
            h.push(0); // external
            for _ in pack_sizes {
                write_number(&mut h, 1); // one coder
                let id_bytes = method_id_bytes(self.method.id());
                h.push(id_bytes.len() as u8); // id size, no attrs, no complex streams
                h.extend_from_slice(&id_bytes);
                // No bind pairs or pack indices: a single 1:1 coder
                // folder encodes none (they are implied).
            }
            h.push(property::CODERS_UNPACK_SIZE as u8);
            for s in unpack_sizes {
                write_number(&mut h, *s);
            }
            // Folder CRCs: all defined (each folder = one file).
            h.push(property::CRC as u8);
            h.push(1); // all-defined bit vector
            for (name, _, _, _, _) in names.iter().filter(|n| n.2) {
                let data = &self.files[name.as_str()].1;
                h.extend_from_slice(&omnizip_archive_core::crc32(data).to_le_bytes());
            }
            h.push(property::END as u8); // end unpack info
            // No substreams info: one stream per folder is implied.
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
        }

        // mtime for every entry.
        h.push(property::MTIME as u8);
        let size = names.len().div_ceil(8) + 1 + names.len() * 8;
        write_number(&mut h, size as u64);
        let all: Vec<bool> = vec![true; names.len()];
        write_bit_vector(&mut h, &all);
        h.push(0); // external
        for _ in names {
            h.extend_from_slice(&crate::unix_to_filetime(options.mtime).to_le_bytes());
        }

        // Windows attributes: unix mode in the high 16 bits (the
        // 7-Zip-on-unix convention) + FILE_ATTRIBUTE_DIRECTORY.
        h.push(property::WIN_ATTRIB as u8);
        let size = names.len().div_ceil(8) + 1 + names.len() * 4;
        write_number(&mut h, size as u64);
        write_bit_vector(&mut h, &all);
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

    fn build(m: SevenZipMethod) -> Vec<u8> {
        let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
        let mut w = SevenZipWriter::new(m);
        w.add_directory(&NewEntry::directory("doc", &opts), &opts).unwrap();
        w.add_file(
            &NewEntry::file("doc/readme.txt", &opts),
            b"seven zip round trip\n".repeat(20).as_slice(),
            &opts,
        )
        .unwrap();
        w.add_file(&NewEntry::file("doc/data.bin", &opts), &[0x77; 1024], &opts)
            .unwrap();
        w.finish_bytes(&opts).unwrap()
    }

    #[test]
    fn round_trips_all_methods() {
        for m in [
            SevenZipMethod::Copy,
            SevenZipMethod::Deflate,
            SevenZipMethod::Bzip2,
        ] {
            let bytes = build(m);
            let mut r = SevenZipReader::from_bytes(&bytes).unwrap();
            let entries = r.entries().unwrap();
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            assert_eq!(names, vec!["doc", "doc/data.bin", "doc/readme.txt"], "{m:?}");
            let readme = names.iter().position(|n| *n == "doc/readme.txt").unwrap();
            assert_eq!(
                r.read_entry(readme).unwrap(),
                b"seven zip round trip\n".repeat(20),
                "{m:?}"
            );
            let bin = names.iter().position(|n| *n == "doc/data.bin").unwrap();
            assert_eq!(r.read_entry(bin).unwrap(), vec![0x77; 1024], "{m:?}");
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(build(SevenZipMethod::Deflate), build(SevenZipMethod::Deflate));
    }

    #[test]
    fn vli_round_trip() {
        use crate::parser::HeaderParser;
        for value in [0u64, 1, 0x7F, 0x80, 0xFF, 0x100, 0x1234, 0xFFFF, 0x1_0000, u64::from(u32::MAX), 0x00FF_FFFF_FFFF_FFFF] {
            let mut buf = Vec::new();
            write_number(&mut buf, value);
            let mut p = HeaderParser::new(&buf);
            assert_eq!(p.number().unwrap(), value, "value {value:x} -> {:02x?}", buf);
        }
    }
}
