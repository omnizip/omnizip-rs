//! Property-encoded header parser — port of the Ruby `parser.rb`:
//! 7-Zip VLI numbers, bit vectors, and the pack/unpack/substreams/
//! files-info sections.
#![forbid(unsafe_code)]

use crate::{property, CoderInfo, FileEntry, Folder, StreamInfo, START_HEADER_SIZE};
use omnizip_archive_core::ArchiveError;

/// Byte-cursor over the (decoded) header.
pub struct HeaderParser<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> HeaderParser<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    #[must_use]
    pub const fn eof(&self) -> bool {
        self.position >= self.data.len()
    }

    fn byte(&mut self) -> Result<u8, ArchiveError> {
        let b = *self
            .data
            .get(self.position)
            .ok_or_else(|| ArchiveError::InvalidArchive("7z: header truncated".into()))?;
        self.position += 1;
        Ok(b)
    }

    #[must_use]
    pub fn peek(&self) -> Option<u8> {
        self.data.get(self.position).copied()
    }

    /// 7-Zip VLI number (extra bytes little-endian, per the SDK's
    /// `ReadNumber`).
    pub fn number(&mut self) -> Result<u64, ArchiveError> {
        let first = self.byte()?;
        if first & 0x80 == 0 {
            return Ok(u64::from(first));
        }
        let mut mask = 0x80u8;
        let mut value: u64 = 0;
        let mut shift = 0u32;
        while first & mask != 0 {
            value |= u64::from(self.byte()?) << shift;
            shift += 8;
            mask >>= 1;
        }
        let data_bits = if mask == 0 { 0 } else { first & (mask - 1) };
        if data_bits != 0 {
            value |= u64::from(data_bits) << shift;
        }
        Ok(value)
    }

    pub fn uint32(&mut self) -> Result<u32, ArchiveError> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes(b.try_into().expect("4")))
    }

    pub fn uint64(&mut self) -> Result<u64, ArchiveError> {
        let b = self.bytes(8)?;
        Ok(u64::from_le_bytes(b.try_into().expect("8")))
    }

    fn bytes(&mut self, count: usize) -> Result<&'a [u8], ArchiveError> {
        let s = self
            .data
            .get(self.position..self.position + count)
            .ok_or_else(|| ArchiveError::InvalidArchive("7z: header truncated".into()))?;
        self.position += count;
        Ok(s)
    }

    fn skip(&mut self, count: usize) {
        self.position = (self.position + count).min(self.data.len());
    }

    /// Plain bit vector: MSB-first packed bits, no marker (the shape
    /// `ReadBoolVector` reads in 7zIn.cpp).
    fn bit_vector(&mut self, num_items: usize) -> Result<Vec<u8>, ArchiveError> {
        let num_bytes = num_items.div_ceil(8);
        let bits = self.bytes(num_bytes)?;
        let mut out = Vec::with_capacity(num_items);
        for i in 0..num_items {
            out.push((bits[i / 8] >> (7 - (i % 8))) & 1);
        }
        Ok(out)
    }

    /// Digest-defined vector: an all-defined byte, then packed bits
    /// when it is zero (`ReadBoolVector2`).
    fn digest_defined_vector(&mut self, num_items: usize) -> Result<Vec<u8>, ArchiveError> {
        let all = self.byte()?;
        if all != 0 {
            return Ok(vec![1; num_items]);
        }
        self.bit_vector(num_items)
    }

    fn skip_data(&mut self) -> Result<(), ArchiveError> {
        let size = self.number()? as usize;
        self.skip(size);
        Ok(())
    }

    /// kPackInfo.
    pub fn pack_info(&mut self, info: &mut StreamInfo) -> Result<(), ArchiveError> {
        info.pack_pos = self.number()?;
        let num_pack = self.number()? as usize;
        let mut sizes_read = false;
        while !self.eof() && self.peek() != Some(property::END as u8) {
            match self.peek() {
                Some(p) if u64::from(p) == property::SIZE => {
                    self.byte()?;
                    for _ in 0..num_pack {
                        info.pack_sizes.push(self.number()?);
                    }
                    sizes_read = true;
                }
                Some(p) if u64::from(p) == property::CRC => {
                    self.byte()?;
                    let defined = self.digest_defined_vector(num_pack)?;
                    for d in defined {
                        // Pack-stream CRCs are optional and unused by
                        // the extractor, but must be consumed.
                        if d != 0 {
                            let _ = self.uint32()?;
                        }
                    }
                }
                _ => {
                    self.byte()?;
                    self.skip_data()?;
                }
            }
        }
        if !sizes_read {
            return Err(ArchiveError::InvalidArchive(
                "7z: pack info missing SIZE".into(),
            ));
        }
        if self.peek() == Some(property::END as u8) {
            self.byte()?;
        }
        Ok(())
    }

    /// kUnpackInfo (folders + sizes [+ crcs]).
    pub fn unpack_info(&mut self, info: &mut StreamInfo) -> Result<(), ArchiveError> {
        let mut folders_read = false;
        let mut sizes_read = false;
        while !self.eof() && self.peek() != Some(property::END as u8) {
            let prop = self.byte()?;
            match u64::from(prop) {
                property::FOLDER => {
                    self.folders(info)?;
                    folders_read = true;
                }
                property::CODERS_UNPACK_SIZE => {
                    for folder in &mut info.folders {
                        let n = folder
                            .coders
                            .iter()
                            .map(|c| c.num_out_streams)
                            .sum::<u64>()
                            .max(1);
                        for _ in 0..n {
                            folder.unpack_sizes.push(self.number()?);
                        }
                    }
                    sizes_read = true;
                }
                property::CRC => {
                    let n = info.folders.len();
                    let defined = self.digest_defined_vector(n)?;
                    for (i, d) in defined.iter().enumerate() {
                        if *d != 0 {
                            info.folders[i].unpack_crc = Some(self.uint32()?);
                        }
                    }
                }
                _ => self.skip_data()?,
            }
        }
        if !folders_read {
            return Err(ArchiveError::InvalidArchive(
                "7z: unpack info missing FOLDER".into(),
            ));
        }
        if !sizes_read {
            return Err(ArchiveError::InvalidArchive(
                "7z: unpack info missing CODERS_UNPACK_SIZE".into(),
            ));
        }
        if self.peek() == Some(property::END as u8) {
            self.byte()?;
        }
        Ok(())
    }

    fn folders(&mut self, info: &mut StreamInfo) -> Result<(), ArchiveError> {
        let num_folders = self.number()? as usize;
        let external = self.byte()?;
        if external != 0 {
            return Err(ArchiveError::UnsupportedFeature {
                reason: "7z: external folders not supported".into(),
            });
        }
        for _ in 0..num_folders {
            let mut folder = Folder::default();
            self.folder(&mut folder)?;
            info.folders.push(folder);
        }
        Ok(())
    }

    fn folder(&mut self, folder: &mut Folder) -> Result<(), ArchiveError> {
        let num_coders = self.number()?;
        if num_coders > 64 {
            return Err(ArchiveError::InvalidArchive("7z: too many coders".into()));
        }
        for _ in 0..num_coders {
            let main = self.byte()?;
            let id_size = (main & 0x0F) as usize;
            let has_attributes = main & 0x20 != 0;
            let complex = main & 0x10 != 0;

            let mut method_id: u64 = 0;
            for _ in 0..id_size {
                method_id = (method_id << 8) | u64::from(self.byte()?);
            }
            let (in_s, out_s) = if complex {
                (self.number()?, self.number()?)
            } else {
                (1, 1)
            };
            let properties = if has_attributes {
                let size = self.number()? as usize;
                self.bytes(size)?.to_vec()
            } else {
                Vec::new()
            };
            folder.coders.push(CoderInfo {
                method_id,
                num_in_streams: in_s,
                num_out_streams: out_s,
                properties,
            });
        }

        let num_out: u64 = folder.coders.iter().map(|c| c.num_out_streams).sum();
        let num_in: u64 = folder.coders.iter().map(|c| c.num_in_streams).sum();
        let num_bind_pairs = num_out.saturating_sub(1);
        for _ in 0..num_bind_pairs {
            let i = self.number()?;
            let o = self.number()?;
            folder.bind_pairs.push((i, o));
        }
        let num_pack_streams = num_in - num_bind_pairs;
        if num_pack_streams == 1 {
            for i in 0..num_in {
                if !folder.bind_pairs.iter().any(|&(in_i, _)| in_i == i) {
                    folder.pack_stream_indices.push(i);
                    break;
                }
            }
        } else {
            for _ in 0..num_pack_streams {
                folder.pack_stream_indices.push(self.number()?);
            }
        }
        Ok(())
    }

    /// kSubstreamsInfo.
    pub fn substreams_info(&mut self, info: &mut StreamInfo) -> Result<(), ArchiveError> {
        if self.peek() == Some(property::NUM_UNPACK_STREAM as u8) {
            self.byte()?;
            for _ in 0..info.folders.len() {
                info.num_unpack_streams_in_folders.push(self.number()?);
            }
        } else {
            info.num_unpack_streams_in_folders = vec![1; info.folders.len()];
        }

        if self.peek() == Some(property::SIZE as u8) {
            self.byte()?;
            for (fi, folder) in info.folders.iter().enumerate() {
                let num_streams = info.num_unpack_streams_in_folders[fi];
                let start = info.unpack_sizes.len();
                for _ in 0..num_streams.saturating_sub(1) {
                    info.unpack_sizes.push(self.number()?);
                }
                let sum: u64 = info.unpack_sizes[start..].iter().sum();
                let total = folder.uncompressed_size();
                info.unpack_sizes.push(total.saturating_sub(sum));
            }
        } else {
            for (fi, folder) in info.folders.iter().enumerate() {
                if info.num_unpack_streams_in_folders[fi] == 1 {
                    let size = folder.unpack_sizes.iter().sum();
                    info.unpack_sizes.push(size);
                }
            }
        }

        let num_digests: usize = info
            .num_unpack_streams_in_folders
            .iter()
            .map(|n| *n as usize)
            .sum();
        if self.peek() == Some(property::CRC as u8) {
            self.byte()?;
            let defined = self.digest_defined_vector(num_digests)?;
            for d in defined {
                info.digests
                    .push(if d != 0 { Some(self.uint32()?) } else { None });
            }
        }
        if self.peek() == Some(property::END as u8) {
            self.byte()?;
        }
        Ok(())
    }

    /// kFilesInfo → file entries.
    pub fn files_info(&mut self) -> Result<Vec<FileEntry>, ArchiveError> {
        let num_files = self.number()? as usize;
        // Spec default: without kEmptyStream every entry has a stream.
        let mut entries = (0..num_files)
            .map(|_| FileEntry {
                has_stream: true,
                ..FileEntry::default()
            })
            .collect::<Vec<_>>();

        while !self.eof() && self.peek() != Some(property::END as u8) {
            let prop = self.byte()?;
            match u64::from(prop) {
                property::NAME => self.names(&mut entries)?,
                property::EMPTY_STREAM => {
                    let _size = self.number()?;
                    let bits = self.bit_vector(num_files)?;
                    for (entry, bit) in entries.iter_mut().zip(bits) {
                        entry.has_stream = bit == 0;
                        entry.is_dir = bit == 1;
                    }
                }
                property::EMPTY_FILE => {
                    let _size = self.number()?;
                    let empties: Vec<usize> = entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| !e.has_stream)
                        .map(|(i, _)| i)
                        .collect();
                    let bits = self.bit_vector(empties.len())?;
                    for (i, bit) in empties.into_iter().zip(bits) {
                        entries[i].is_empty = bit == 0;
                    }
                }
                property::ANTI => {
                    let _size = self.number()?;
                    let antis: Vec<usize> = entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| !e.has_stream && !e.is_empty)
                        .map(|(i, _)| i)
                        .collect();
                    let bits = self.bit_vector(antis.len())?;
                    for (i, bit) in antis.into_iter().zip(bits) {
                        entries[i].is_anti = bit != 0;
                    }
                }
                property::MTIME => self.timestamps(&mut entries)?,
                property::CTIME | property::ATIME => self.timestamps(&mut entries)?,
                property::WIN_ATTRIB => {
                    let _size = self.number()?;
                    let defined = self.bit_vector(num_files)?;
                    let external = self.byte()?;
                    if external == 0 {
                        for (entry, d) in entries.iter_mut().zip(defined) {
                            if d != 0 {
                                entry.attributes = Some(self.uint32()?);
                            }
                        }
                    }
                }
                _ => self.skip_data()?,
            }
        }
        if self.peek() == Some(property::END as u8) {
            self.byte()?;
        }
        Ok(entries)
    }

    fn names(&mut self, entries: &mut [FileEntry]) -> Result<(), ArchiveError> {
        let size = self.number()? as usize;
        let start = self.position;
        let external = self.byte()?;
        if external == 0 {
            for entry in entries.iter_mut() {
                let mut units = Vec::new();
                loop {
                    let lo = self.byte()?;
                    let hi = self.byte()?;
                    if lo == 0 && hi == 0 {
                        break;
                    }
                    units.push(u16::from(lo) | (u16::from(hi) << 8));
                }
                entry.name = String::from_utf16_lossy(&units);
            }
        }
        let consumed = self.position - start;
        if consumed < size {
            self.skip(size - consumed);
        }
        Ok(())
    }

    fn timestamps(&mut self, entries: &mut [FileEntry]) -> Result<(), ArchiveError> {
        let _size = self.number()?;
        let defined = self.bit_vector(entries.len())?;
        let external = self.byte()?;
        if external == 0 {
            for (entry, d) in entries.iter_mut().zip(defined) {
                if d != 0 {
                    let ft = self.uint64()?;
                    entry.mtime = Some(crate::filetime_to_unix(ft));
                }
            }
        }
        Ok(())
    }
}

/// Parse a complete next-header (already decoded): HEADER property →
/// streams info + files info. Returns (stream_info, entries).
pub fn parse_metadata(data: &[u8]) -> Result<(StreamInfo, Vec<FileEntry>), ArchiveError> {
    let mut p = HeaderParser::new(data);
    let first = p
        .peek()
        .ok_or_else(|| ArchiveError::InvalidArchive("7z: empty metadata".into()))?;
    if u64::from(first) != property::HEADER {
        return Err(ArchiveError::InvalidArchive(format!(
            "7z: expected HEADER property, got 0x{first:02x}"
        )));
    }
    p.byte()?;

    let mut info = StreamInfo::default();
    let mut entries = Vec::new();
    while !p.eof() {
        let prop = p.byte()?;
        match u64::from(prop) {
            property::MAIN_STREAMS_INFO => {
                info = streams_info(&mut p)?;
            }
            property::FILES_INFO => entries = p.files_info()?,
            property::END => break,
            _ => {
                if p.peek() != Some(property::END as u8) {
                    p.skip_data()?;
                }
            }
        }
    }
    Ok((info, entries))
}

/// Parse a streams-info block (used for the main streams and encoded
/// headers alike).
pub fn streams_info(p: &mut HeaderParser) -> Result<StreamInfo, ArchiveError> {
    let mut info = StreamInfo::default();
    while !p.eof() {
        let prop = p.peek().unwrap_or(0);
        match u64::from(prop) {
            property::PACK_INFO => {
                p.byte()?;
                p.pack_info(&mut info)?;
            }
            property::UNPACK_INFO => {
                p.byte()?;
                p.unpack_info(&mut info)?;
            }
            property::SUBSTREAMS_INFO => {
                p.byte()?;
                p.substreams_info(&mut info)?;
            }
            property::END => {
                p.byte()?;
                break;
            }
            _ => {
                p.byte()?;
                p.skip_data()?;
            }
        }
    }
    // Spec default: no kSubstreamsInfo means exactly one stream per
    // folder, sized by the folder's unpack size.
    if info.num_unpack_streams_in_folders.is_empty() {
        info.num_unpack_streams_in_folders = vec![1; info.folders.len()];
        for folder in &info.folders {
            let size = folder.unpack_sizes.iter().sum();
            info.unpack_sizes.push(size);
        }
    }
    Ok(info)
}

/// Parse streams info when it is the top-level property (encoded
/// headers): the wrapper byte may be MAIN_STREAMS_INFO or directly
/// PACK_INFO.
pub fn streams_info_top(data: &[u8]) -> Result<StreamInfo, ArchiveError> {
    let mut p = HeaderParser::new(&data[1..]); // skip ENCODED_HEADER marker
    let first = p
        .peek()
        .ok_or_else(|| ArchiveError::InvalidArchive("7z: empty encoded header".into()))?;
    if u64::from(first) == property::MAIN_STREAMS_INFO {
        p.byte()?;
        streams_info(&mut p)
    } else if u64::from(first) == property::PACK_INFO {
        streams_info(&mut p)
    } else {
        Err(ArchiveError::InvalidArchive(format!(
            "7z: unexpected property 0x{first:02x} in encoded header"
        )))
    }
}

/// Size hint for tests.
#[must_use]
pub const fn start_header_size() -> usize {
    START_HEADER_SIZE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vli_numbers() {
        // 1-byte, then a multi-byte encoding.
        let data = [0x7F, 0x80 | 0x02, 0x34];
        let mut p = HeaderParser::new(&data);
        assert_eq!(p.number().unwrap(), 0x7F);
        // 0x82: mask 0x80 set -> 1 extra byte (0x34); data bits =
        // 0x82 & 0x3F = 2 at the high position -> 0x234.
        assert_eq!(p.number().unwrap(), 0x234);
    }
}
