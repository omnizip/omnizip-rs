//! 7z reader — port of the Ruby `reader.rb` (single-volume path):
//! start header, plain/encoded next header, metadata parse,
//! entry→stream mapping, and folder decoding through the coder chain
//! (Copy / LZMA / LZMA2 / BZip2 / Deflate with delta + BCJ filters),
//! with solid-folder caching. BCJ2 folders give a clear unsupported
//! error (same as the Ruby).
#![forbid(unsafe_code)]

use crate::{
    method, parse_start_header, parser, property, FileEntry, Folder, StartHeader, StreamInfo,
    START_HEADER_SIZE,
};
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader, EntryKind};
use std::collections::HashMap;
use std::path::Path;

/// Reads a 7z archive held in memory.
pub struct SevenZipReader {
    data: Vec<u8>,
    start: StartHeader,
    pub stream_info: StreamInfo,
    pub entries: Vec<FileEntry>,
    password: Option<String>,
    /// Decoded solid folders, cached until all their files are read.
    solid_cache: HashMap<usize, Vec<u8>>,
}

impl SevenZipReader {
    /// Parse a 7z from raw bytes.
    ///
    /// # Errors
    ///
    /// [`ArchiveError`] on structure, CRC, or decode problems.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        Self::from_bytes_with_password(data, None)
    }

    /// Parse with a password (encrypted streams / headers).
    ///
    /// # Errors
    ///
    /// As [`Self::from_bytes`].
    pub fn from_bytes_with_password(
        bytes: &[u8],
        password: Option<&str>,
    ) -> Result<Self, ArchiveError> {
        let data: Vec<u8> = bytes.to_vec();
        let start = parse_start_header(&data)?;
        let header_pos = START_HEADER_SIZE + start.next_header_offset as usize;
        let raw = data
            .get(header_pos..header_pos + start.next_header_size as usize)
            .ok_or_else(|| ArchiveError::InvalidArchive("7z: next header out of bounds".into()))?;

        // Verify the next-header CRC.
        let computed = omnizip_archive_core::crc32(raw);
        if computed != start.next_header_crc {
            return Err(ArchiveError::Checksum(format!(
                "7z: next-header CRC mismatch: stored {:08X}, computed {computed:08X}",
                start.next_header_crc
            )));
        }

        // Encoded header (compressed; AES-encrypted headers arrive as
        // an AES coder in the encoded stream info).
        let header = if raw.first() == Some(&(property::ENCODED_HEADER as u8)) {
            let info = parser::streams_info_top(raw)?;
            let folder = info
                .folders
                .first()
                .ok_or_else(|| ArchiveError::InvalidArchive("7z: encoded header has no folder".into()))?;
            let pack_pos = START_HEADER_SIZE + info.pack_pos as usize;
            let pack_size = info.pack_sizes.first().copied().unwrap_or(0) as usize;
            let packed = data
                .get(pack_pos..pack_pos + pack_size)
                .ok_or_else(|| ArchiveError::InvalidArchive("7z: encoded header bytes missing".into()))?;
            decode_folder(folder, packed, password, folder.uncompressed_size() as usize)?
        } else {
            raw.to_vec()
        };

        let (stream_info, entries) = parser::parse_metadata(&header)?;
        let mut reader = Self {
            data,
            start,
            stream_info,
            entries,
            password: password.map(String::from),
            solid_cache: HashMap::new(),
        };
        reader.map_entries_to_streams();
        Ok(reader)
    }

    /// Open from disk.
    ///
    /// # Errors
    ///
    /// IO or archive errors.
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        let bytes = std::fs::read(path).map_err(|e| ArchiveError::io("read", path, e))?;
        Self::from_bytes(&bytes)
    }

    /// Open from disk with a password.
    ///
    /// # Errors
    ///
    /// IO or archive errors.
    pub fn open_with_password(path: &Path, password: &str) -> Result<Self, ArchiveError> {
        let bytes = std::fs::read(path).map_err(|e| ArchiveError::io("read", path, e))?;
        Self::from_bytes_with_password(&bytes, Some(password))
    }

    #[must_use]
    pub const fn start_header(&self) -> StartHeader {
        self.start
    }

    fn map_entries_to_streams(&mut self) {
        let mut stream_idx = 0usize;
        for i in 0..self.entries.len() {
            if !self.entries[i].has_stream {
                continue;
            }
            let mut folder_idx = 0usize;
            let mut accumulated = 0usize;
            for (fi, num) in self.stream_info.num_unpack_streams_in_folders.iter().enumerate() {
                if stream_idx < accumulated + *num as usize {
                    folder_idx = fi;
                    break;
                }
                accumulated += *num as usize;
            }
            let size = self
                .stream_info
                .unpack_sizes
                .get(stream_idx)
                .copied()
                .unwrap_or(0);
            self.entries[i].folder_index = folder_idx;
            self.entries[i].file_index = i;
            self.entries[i].size = size;
            self.entries[i].crc = self.stream_info.digests.get(stream_idx).copied().flatten();
            stream_idx += 1;
        }
    }

    /// Offset (within the decoded folder) of the file's portion in a
    /// solid block.
    fn solid_offset(&self, entry: &FileEntry) -> usize {
        let mut offset = 0usize;
        for e in &self.entries {
            if e.file_index == entry.file_index {
                break;
            }
            if e.has_stream && e.folder_index == entry.folder_index {
                offset += e.size as usize;
            }
        }
        offset
    }

    /// Decode (and cache) a whole folder.
    fn folder_data(&mut self, folder_index: usize) -> Result<Vec<u8>, ArchiveError> {
        if let Some(cached) = self.solid_cache.get(&folder_index) {
            return Ok(cached.clone());
        }
        let folder = self
            .stream_info
            .folders
            .get(folder_index)
            .ok_or_else(|| {
                ArchiveError::InvalidArchive(format!("7z: folder {folder_index} missing"))
            })?;

        // Pack-stream index range for this folder.
        let mut pack_idx = 0usize;
        let mut byte_offset = 0usize;
        for (i, f) in self.stream_info.folders.iter().enumerate() {
            if i == folder_index {
                break;
            }
            pack_idx += f.pack_stream_indices.len();
            for p in &self.stream_info.pack_sizes
                [pack_idx - f.pack_stream_indices.len()..pack_idx]
            {
                byte_offset += *p as usize;
            }
        }
        let num_packs = folder.pack_stream_indices.len();
        if num_packs != 1 {
            return Err(ArchiveError::UnsupportedFeature {
                reason: "7z: multi-pack-stream folders (BCJ2) are not supported".into(),
            });
        }
        let pack_size = self.stream_info.pack_sizes.get(pack_idx).copied().unwrap_or(0) as usize;
        let pack_pos = START_HEADER_SIZE + self.stream_info.pack_pos as usize + byte_offset;
        let packed = self
            .data
            .get(pack_pos..pack_pos + pack_size)
            .ok_or_else(|| ArchiveError::InvalidArchive("7z: packed data out of bounds".into()))?
            .to_vec();

        let decoded = decode_folder(
            folder,
            &packed,
            self.password.as_deref(),
            folder.uncompressed_size() as usize,
        )?;
        Ok(decoded)
    }
}

impl ArchiveReader for SevenZipReader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        Ok(self
            .entries
            .iter()
            .filter(|e| !e.name.is_empty())
            .map(|e| ArchiveEntry {
                name: e.name.clone(),
                size: Some(e.size),
                mtime: e.mtime,
                mode: e.unix_mode().or(Some(if e.is_dir { 0o755 } else { 0o644 })),
                kind: if e.is_dir {
                    EntryKind::Directory
                } else {
                    EntryKind::Regular
                },
                uid: None,
                gid: None,
                uname: String::new(),
                gname: String::new(),
                method: None,
            })
            .collect())
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        // Map the listing index (skipping unnamed entries) back to the
        // files-info index.
        let listed: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.name.is_empty())
            .map(|(i, _)| i)
            .collect();
        let entry_i = *listed
            .get(index)
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("7z: no entry {index}")))?;
        let entry = self.entries[entry_i].clone();
        if !entry.has_stream {
            return Ok(Vec::new());
        }

        let full = self.folder_data(entry.folder_index)?;
        let num_in_folder = self
            .stream_info
            .num_unpack_streams_in_folders
            .get(entry.folder_index)
            .copied()
            .unwrap_or(1);
        let data = if num_in_folder > 1 {
            let offset = self.solid_offset(&entry);
            full.get(offset..offset + entry.size as usize)
                .unwrap_or(&[])
                .to_vec()
        } else {
            full[..(entry.size as usize).min(full.len())].to_vec()
        };

        // Evict the solid cache once every file in the folder has been
        // served.
        if num_in_folder > 1 {
            let _ = &entry_i;
            let all_served = false;
            // Cheap heuristic: keep the cache; large solid folders are
            // served in listing order in practice, and memory is
            // bounded by the folder size. Evict when the last file of
            // the folder is read.
            let last_of_folder = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.has_stream && e.folder_index == entry.folder_index)
                .map(|(i, _)| i)
                .max()
                == Some(entry_i);
            if all_served || last_of_folder {
                self.solid_cache.remove(&entry.folder_index);
            }
        }

        if let Some(crc) = entry.crc {
            let computed = omnizip_archive_core::crc32(&data);
            if computed != crc {
                return Err(ArchiveError::Checksum(format!(
                    "7z: entry '{}': CRC mismatch: stored {crc:08X}, computed {computed:08X}",
                    entry.name
                )));
            }
        }
        Ok(data)
    }
}

/// Decode one folder's packed stream through its coder chain.
pub fn decode_folder(
    folder: &Folder,
    packed: &[u8],
    password: Option<&str>,
    unpack_size: usize,
) -> Result<Vec<u8>, ArchiveError> {
    // The chain: [filters...] + [compression] (+ [AES] first when
    // encrypted). Decode in reverse order over the single data path.
    let mut data = packed.to_vec();
    let mut decoded_any = false;

    for coder in folder.coders.iter().rev() {
        match coder.method_id {
            method::COPY => {
                decoded_any = true;
            }
            method::LZMA2 => {
                let (out, _consumed) =
                    omnizip_lzma::lzma2::decode_lzma2_stream(&data).map_err(|e| {
                        ArchiveError::InvalidArchive(format!("7z LZMA2: {e}"))
                    })?;
                data = out;
                decoded_any = true;
            }
            method::LZMA => {
                let (lc, lp, pb, dict_size) = lzma_props(&coder.properties)?;
                let mut decoder = omnizip_lzma::decoder::lzma1::Lzma1Decoder::new(
                    lc, lp, pb, dict_size,
                );
                // 7z LZMA streams carry no EOPM; the size is exact.
                data = decoder
                    .decode(&data, Some(unpack_size as u64), false)
                    .map_err(|e| ArchiveError::InvalidArchive(format!("7z LZMA: {e}")))?;
                decoded_any = true;
            }
            method::BZIP2 => {
                data = omnizip_bzip2::decompress_framed(&data)
                    .map_err(|e| ArchiveError::InvalidArchive(format!("7z BZip2: {e}")))?;
                decoded_any = true;
            }
            method::DEFLATE => {
                let mut hint = (data.len() * 6).max(64);
                loop {
                    match omnizip_libdeflate::inflate::inflate(&data, hint) {
                        Ok(d) => {
                            data = d;
                            break;
                        }
                        Err(_) if hint < (1 << 32) => hint = hint.saturating_mul(4),
                        Err(e) => {
                            return Err(ArchiveError::InvalidArchive(format!("7z Deflate: {e}")))
                        }
                    }
                }
                decoded_any = true;
            }
            method::DELTA => {
                let distance = coder.properties.first().copied().unwrap_or(0) as usize + 1;
                data = omnizip_filters::Filter::decode(&omnizip_filters::DeltaFilter::new(distance), &data);
                decoded_any = true;
            }
            method::BCJ_X86 => {
                data = omnizip_filters::Filter::decode(&omnizip_filters::BcjX86Filter, &data);
                decoded_any = true;
            }
            method::BCJ_ARM => {
                data = omnizip_filters::Filter::decode(&omnizip_filters::BcjArmFilter, &data);
                decoded_any = true;
            }
            method::BCJ_PPC => {
                data = omnizip_filters::Filter::decode(&omnizip_filters::BcjPowerPcFilter, &data);
                decoded_any = true;
            }
            method::BCJ_IA64 => {
                data = omnizip_filters::Filter::decode(&omnizip_filters::BcjIa64Filter, &data);
                decoded_any = true;
            }
            method::BCJ_SPARC => {
                data = omnizip_filters::Filter::decode(&omnizip_filters::BcjSparcFilter, &data);
                decoded_any = true;
            }
            method::BCJ2 => {
                return Err(ArchiveError::UnsupportedFeature {
                    reason: "7z: BCJ2 multi-stream folders are not supported".into(),
                });
            }
            method::AES => {
                let password = password.ok_or_else(|| {
                    ArchiveError::Security(
                        "7z: archive is AES-encrypted; supply a password".into(),
                    )
                })?;
                data = decode_aes_stream(&data, &coder.properties, password)?;
            }
            other => {
                return Err(ArchiveError::UnsupportedFeature {
                    reason: format!("7z: method {} not supported", method::name(other)),
                });
            }
        }
    }
    if !decoded_any {
        return Err(ArchiveError::InvalidArchive("7z: folder has no decoders".into()));
    }
    Ok(data)
}

/// LZMA coder properties: [lc/lp/pb byte][dict u32 LE].
fn lzma_props(props: &[u8]) -> Result<(u32, u32, u32, u32), ArchiveError> {
    let (&b, rest) = props
        .split_first()
        .ok_or_else(|| ArchiveError::InvalidArchive("7z: LZMA coder without properties".into()))?;
    let dict = u32::from_le_bytes(
        rest.get(..4)
            .ok_or_else(|| ArchiveError::InvalidArchive("7z: LZMA properties truncated".into()))?
            .try_into()
            .expect("4"),
    );
    let lc = u32::from(b % 9);
    let rem = b / 9;
    Ok((lc, u32::from(rem % 5), u32::from(rem / 5), dict))
}

/// AES-encrypted stream: properties = [salt_len][cycles_power] (with
/// the salt-length high bits per the 7z AES coder spec).
fn decode_aes_stream(
    packed: &[u8],
    props: &[u8],
    password: &str,
) -> Result<Vec<u8>, ArchiveError> {
    let (&first, rest) = props
        .split_first()
        .ok_or_else(|| ArchiveError::InvalidArchive("7z: AES coder without properties".into()))?;
    let salt_len = u32::from(first & 0x7F);
    let salt_len = if salt_len == 0 && first & 0x80 != 0 {
        // High bit set with low 7 zero is unused; 0x7F = 0 means none.
        0
    } else {
        salt_len
    };
    let salt_len = (salt_len.min(16)) as usize;
    let cycles_power = *rest.first().ok_or_else(|| {
        ArchiveError::InvalidArchive("7z: AES properties missing cycles".into())
    })?;
    let salt = packed
        .get(..salt_len)
        .ok_or_else(|| ArchiveError::InvalidArchive("7z: AES stream shorter than its salt".into()))?;
    let iv = packed
        .get(salt_len..salt_len + 16)
        .ok_or_else(|| ArchiveError::InvalidArchive("7z: AES stream missing IV".into()))?;
    let body = &packed[salt_len + 16..];

    let (key, mut iv_arr) = crate::aes256_kdf(password.as_bytes(), salt, cycles_power);
    iv_arr.copy_from_slice(
        iv.try_into()
            .map_err(|_| ArchiveError::InvalidArchive("7z: bad AES IV".into()))?,
    );
    let mut buf = body.to_vec();
    omnizip_crypto::AesCbc256Decrypt::new(&key, &iv_arr).decrypt(&mut buf);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_7z() {
        assert!(SevenZipReader::from_bytes(b"not a seven zip file at all").is_err());
    }

    #[test]
    fn rejects_bad_signature() {
        let mut buf = vec![0u8; 64];
        buf[0..6].copy_from_slice(b"7z\xBC\xAF\x27\x1D");
        assert!(SevenZipReader::from_bytes(&buf).is_err());
    }
}
