//! RPM reader — port of the Ruby `formats/rpm.rb` Reader: lead +
//! signature header (8-aligned) + main header, then the compressed
//! CPIO payload decoded through the registered codecs. File paths
//! come from the basenames/dirindexes/dirnames triple.
#![forbid(unsafe_code)]

use crate::{parse_header, parse_lead, tags, Lead, RpmHeader, HEADER_SIGNED_TYPE};
use omnizip_archive_core::{ArchiveEntry, ArchiveError, ArchiveReader};
use omnizip_cpio::CpioReader;
use std::path::Path;

/// Reads an RPM package held in memory.
pub struct RpmReader {
    data: Vec<u8>,
    pub lead: Lead,
    pub signature: Option<RpmHeader>,
    pub header: RpmHeader,
    payload_offset: usize,
    /// Lazily decompressed cpio: (entry list, per-index data).
    payload: Option<(Vec<ArchiveEntry>, Vec<Vec<u8>>)>,
}

impl RpmReader {
    /// Parse an RPM from raw bytes.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::InvalidArchive`] on structure problems.
    pub fn from_bytes(data: &[u8]) -> Result<Self, ArchiveError> {
        let lead = parse_lead(data)?;
        let mut offset = crate::LEAD_SIZE;

        let mut signature = None;
        if lead.signature_type == HEADER_SIGNED_TYPE {
            let (sig, len) = parse_header(data, offset)?;
            let pad = (8 - (len % 8)) % 8;
            offset += len + pad;
            signature = Some(sig);
        }
        let (header, hlen) = parse_header(data, offset)?;
        offset += hlen;

        Ok(Self {
            data: data.to_vec(),
            lead,
            signature,
            header,
            payload_offset: offset,
            payload: None,
        })
    }

    /// Open an RPM from disk.
    ///
    /// # Errors
    ///
    /// IO or structure errors.
    pub fn open(path: &Path) -> Result<Self, ArchiveError> {
        let data = std::fs::read(path).map_err(|e| ArchiveError::io("read", path, e))?;
        Self::from_bytes(&data)
    }

    /// Package metadata (`ozip`-facing summary of the Ruby `info`).
    #[must_use]
    pub fn package_info(&self) -> PackageInfo {
        let get_str = |t: u32| {
            self.header
                .get(t)
                .and_then(crate::TagValue::as_str)
                .unwrap_or("")
                .to_string()
        };
        PackageInfo {
            name: get_str(tags::NAME),
            version: get_str(tags::VERSION),
            release: get_str(tags::RELEASE),
            arch: get_str(tags::ARCH),
            summary: get_str(tags::SUMMARY),
            description: get_str(tags::DESCRIPTION),
            license: get_str(tags::LICENSE),
            vendor: get_str(tags::VENDOR),
            build_time: self
                .header
                .get(tags::BUILDTIME)
                .and_then(crate::TagValue::as_u32)
                .unwrap_or(0),
        }
    }

    /// The payload compressor name tag ("gzip", "bzip2", "xz",
    /// "zstd"). When the tag is absent (uncompressed payloads), the
    /// raw bytes are sniffed — falling back to raw CPIO.
    #[must_use]
    pub fn payload_compressor(&self) -> String {
        if let Some(name) = self
            .header
            .get(tags::PAYLOADCOMPRESSOR)
            .and_then(crate::TagValue::as_str)
        {
            return name.to_string();
        }
        let raw = self.raw_payload();
        match omnizip_archive_core::detect::detect_format(raw) {
            omnizip_archive_core::detect::FormatKind::Gzip => "gzip".into(),
            omnizip_archive_core::detect::FormatKind::Bzip2 => "bzip2".into(),
            omnizip_archive_core::detect::FormatKind::Xz => "xz".into(),
            omnizip_archive_core::detect::FormatKind::Zstd => "zstd".into(),
            _ => "none".into(),
        }
    }

    fn load_payload(&mut self) -> Result<(), ArchiveError> {
        if self.payload.is_some() {
            return Ok(());
        }
        let raw = self
            .data
            .get(self.payload_offset..)
            .ok_or_else(|| ArchiveError::InvalidArchive("RPM payload missing".into()))?;
        let decompressed = match self.payload_compressor().as_str() {
            "gzip" => omnizip_archive_core::formats::gzip::decompress(raw)
                .map_err(|e| ArchiveError::InvalidArchive(format!("gzip payload: {e}")))?,
            "bzip2" => omnizip_archive_core::formats::bzip2_file::decompress(raw)
                .map_err(|e| ArchiveError::InvalidArchive(format!("bzip2 payload: {e}")))?,
            "xz" | "lzma" => omnizip_lzma::xz_decompress(raw)
                .map_err(|e| ArchiveError::InvalidArchive(format!("xz payload: {e}")))?,
            "zstd" => omnizip_zstd::decompress(raw, u32::MAX)
                .map_err(|e| ArchiveError::InvalidArchive(format!("zstd payload: {e}")))?,
            "none" => raw.to_vec(),
            other => {
                return Err(ArchiveError::UnsupportedFeature {
                    reason: format!("payload compressor '{other}' not supported"),
                });
            }
        };
        let mut cpio = CpioReader::from_bytes(&decompressed)?;
        let entries = cpio.entries()?;
        let mut bodies = Vec::with_capacity(entries.len());
        for i in 0..entries.len() {
            bodies.push(cpio.read_entry(i)?);
        }
        self.payload = Some((entries, bodies));
        Ok(())
    }

    /// Raw (still-compressed) payload bytes.
    #[must_use]
    pub fn raw_payload(&self) -> &[u8] {
        self.data.get(self.payload_offset..).unwrap_or(&[])
    }
}

/// Package metadata summary (the Ruby `Rpm.info` shape).
#[derive(Clone, Debug)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub release: String,
    pub arch: String,
    pub summary: String,
    pub description: String,
    pub license: String,
    pub vendor: String,
    pub build_time: u32,
}

impl ArchiveReader for RpmReader {
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError> {
        self.load_payload()?;
        let (entries, _) = self.payload.as_ref().expect("loaded");
        Ok(entries.clone())
    }

    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError> {
        self.load_payload()?;
        let (_, bodies) = self.payload.as_ref().expect("loaded");
        bodies
            .get(index)
            .cloned()
            .ok_or_else(|| ArchiveError::InvalidArchive(format!("no entry {index}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_rpm() {
        assert!(RpmReader::from_bytes(b"definitely not an rpm").is_err());
    }
}
