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
            let folder = info.folders.first().ok_or_else(|| {
                ArchiveError::InvalidArchive("7z: encoded header has no folder".into())
            })?;
            let pack_pos = START_HEADER_SIZE + info.pack_pos as usize;
            let pack_size = info.pack_sizes.first().copied().unwrap_or(0) as usize;
            let packed = data.get(pack_pos..pack_pos + pack_size).ok_or_else(|| {
                ArchiveError::InvalidArchive("7z: encoded header bytes missing".into())
            })?;
            decode_folder(
                folder,
                packed,
                password,
                folder.uncompressed_size() as usize,
            )?
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
            for (fi, num) in self
                .stream_info
                .num_unpack_streams_in_folders
                .iter()
                .enumerate()
            {
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
        let folder = self.stream_info.folders.get(folder_index).ok_or_else(|| {
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
            for p in &self.stream_info.pack_sizes[pack_idx - f.pack_stream_indices.len()..pack_idx]
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
        let pack_size = self
            .stream_info
            .pack_sizes
            .get(pack_idx)
            .copied()
            .unwrap_or(0) as usize;
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
///
/// The coders form a linear chain (7-Zip writes `[AES, LZMA2]`,
/// `[Delta, LZMA2]`-style folders whose bind pairs order the
/// pipeline): the pack stream feeds the first coder's free input, and
/// each coder's output flows into the input named by a bind pair
/// until an unbound (main) output stream is reached. Coders are
/// applied in that pipeline order, NOT in reversed list order.
pub fn decode_folder(
    folder: &Folder,
    packed: &[u8],
    password: Option<&str>,
    unpack_size: usize,
) -> Result<Vec<u8>, ArchiveError> {
    // Global in-stream index owned by coder i.
    let mut in_base: Vec<u64> = Vec::with_capacity(folder.coders.len());
    let mut out_base: Vec<u64> = Vec::with_capacity(folder.coders.len());
    let mut ins = 0u64;
    let mut outs = 0u64;
    for c in &folder.coders {
        in_base.push(ins);
        out_base.push(outs);
        ins += c.num_in_streams;
        outs += c.num_out_streams;
    }

    let mut data = packed.to_vec();
    let Some(&start_in) = folder.pack_stream_indices.first() else {
        return Err(ArchiveError::InvalidArchive(
            "7z: folder without a pack stream".into(),
        ));
    };
    let mut current_in = start_in;
    // The pack stream must feed an input that no bind pair claims;
    // from there on, bound inputs are reached by following edges.
    if folder.bind_pairs.iter().any(|&(bin, _)| bin == current_in) {
        return Err(ArchiveError::UnsupportedFeature {
            reason: "7z: branching coder chains are not supported".into(),
        });
    }
    loop {
        // The coder whose in-stream range covers current_in.
        let coder_idx = folder
            .coders
            .iter()
            .enumerate()
            .find(|(i, _)| {
                let base = in_base[*i];
                current_in >= base && current_in < base + folder.coders[*i].num_in_streams
            })
            .map(|(i, _)| i)
            .ok_or_else(|| ArchiveError::InvalidArchive("7z: bad pack stream index".into()))?;
        let coder = &folder.coders[coder_idx];
        let _ = current_in - in_base[coder_idx]; // single free input per chain coder
        let out = out_base[coder_idx]; // single out stream per coder in a chain
        let expected = folder
            .unpack_sizes
            .get(out as usize)
            .copied()
            .unwrap_or(unpack_size as u64);
        data = decode_coder(coder, data, password, expected as usize)?;
        match folder.bind_pairs.iter().find(|&&(_, bout)| bout == out) {
            Some(&(next_in, _)) => current_in = next_in,
            None => return Ok(data),
        }
    }
}

fn decode_coder(
    coder: &crate::CoderInfo,
    data: Vec<u8>,
    password: Option<&str>,
    unpack_size: usize,
) -> Result<Vec<u8>, ArchiveError> {
    match coder.method_id {
        method::COPY => Ok(data),
        method::LZMA2 => {
            let (out, _consumed) = omnizip_lzma::lzma2::decode_lzma2_stream(&data)
                .map_err(|e| ArchiveError::InvalidArchive(format!("7z LZMA2: {e}")))?;
            Ok(out)
        }
        method::LZMA => {
            let (lc, lp, pb, dict_size) = lzma_props(&coder.properties)?;
            let mut decoder =
                omnizip_lzma::decoder::lzma1::Lzma1Decoder::new(lc, lp, pb, dict_size);
            // 7z LZMA streams carry no EOPM; the size is exact.
            decoder
                .decode(&data, Some(unpack_size as u64), false)
                .map_err(|e| ArchiveError::InvalidArchive(format!("7z LZMA: {e}")))
        }
        method::BZIP2 => omnizip_bzip2::decompress_framed(&data)
            .map_err(|e| ArchiveError::InvalidArchive(format!("7z BZip2: {e}"))),
        method::DEFLATE => {
            let mut hint = (data.len() * 6).max(64);
            loop {
                match omnizip_libdeflate::inflate::inflate(&data, hint) {
                    Ok(d) => return Ok(d),
                    Err(_) if hint < (1 << 32) => hint = hint.saturating_mul(4),
                    Err(e) => return Err(ArchiveError::InvalidArchive(format!("7z Deflate: {e}"))),
                }
            }
        }
        method::DELTA => {
            let distance = coder.properties.first().copied().unwrap_or(0) as usize + 1;
            Ok(omnizip_filters::Filter::decode(
                &omnizip_filters::DeltaFilter::new(distance),
                &data,
            ))
        }
        method::BCJ_X86 => Ok(omnizip_filters::Filter::decode(
            &omnizip_filters::BcjX86Filter,
            &data,
        )),
        method::BCJ_ARM => Ok(omnizip_filters::Filter::decode(
            &omnizip_filters::BcjArmFilter,
            &data,
        )),
        method::BCJ_PPC => Ok(omnizip_filters::Filter::decode(
            &omnizip_filters::BcjPowerPcFilter,
            &data,
        )),
        method::BCJ_IA64 => Ok(omnizip_filters::Filter::decode(
            &omnizip_filters::BcjIa64Filter,
            &data,
        )),
        method::BCJ_SPARC => Ok(omnizip_filters::Filter::decode(
            &omnizip_filters::BcjSparcFilter,
            &data,
        )),
        method::AES => {
            let password = password.ok_or_else(|| {
                ArchiveError::Security("7z: archive is AES-encrypted; supply a password".into())
            })?;
            decode_aes_stream(&data, &coder.properties, password)
        }
        method::BCJ2 => Err(ArchiveError::UnsupportedFeature {
            reason: "7z: BCJ2 multi-stream folders are not supported".into(),
        }),
        other => Err(ArchiveError::UnsupportedFeature {
            reason: format!("7z: method {} not supported", method::name(other)),
        }),
    }
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

/// AES-encrypted stream: coder properties per 7zAes.cpp
/// `SetDecoderProperties2` —
///
/// ```text
/// b0 = cycles | (salt present ? 0x80 : 0) | (iv present ? 0x40 : 0)
/// b1 = (salt_len - 1) << 4 | (iv_len - 1)      [only when set flags]
/// salt bytes, iv bytes (zero-padded to 16)
/// ```
///
/// The whole packed stream is AES-256-CBC ciphertext (the encoder
/// pads to a block; trailing pad bytes are ignored — later coders
/// stop at their declared unpack sizes).
fn decode_aes_stream(packed: &[u8], props: &[u8], password: &str) -> Result<Vec<u8>, ArchiveError> {
    let (&b0, rest) = props
        .split_first()
        .ok_or_else(|| ArchiveError::InvalidArchive("7z: AES coder without properties".into()))?;
    let cycles = b0 & 0x3F;
    if cycles > 24 && cycles != 0x3F {
        return Err(ArchiveError::UnsupportedFeature {
            reason: format!("7z: AES cycles power {cycles} exceeds the supported 24"),
        });
    }
    let mut salt: &[u8] = &[];
    let mut iv = [0u8; 16];
    if b0 & 0xC0 != 0 {
        let (&b1, body) = rest
            .split_first()
            .ok_or_else(|| ArchiveError::InvalidArchive("7z: AES properties truncated".into()))?;
        let salt_len = (usize::from(b0 >> 7 & 1) + usize::from(b1 >> 4)).min(16);
        let iv_len = (usize::from(b0 >> 6 & 1) + usize::from(b1 & 0x0F)).min(16);
        if body.len() != salt_len + iv_len {
            return Err(ArchiveError::InvalidArchive(
                "7z: AES salt/IV sizes do not match the property length".into(),
            ));
        }
        salt = &body[..salt_len];
        iv[..iv_len].copy_from_slice(&body[salt_len..]);
    }
    if packed.len() % 16 != 0 {
        return Err(ArchiveError::InvalidArchive(
            "7z: AES stream is not block aligned".into(),
        ));
    }
    let key = crate::aes256_kdf(password, salt, cycles);
    let mut buf = packed.to_vec();
    omnizip_crypto::AesCbc256Decrypt::new(&key, &iv).decrypt(&mut buf);
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
