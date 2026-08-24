//! RPM writer — port of the Ruby `formats/rpm/writer.rb` with the
//! task-17 determinism rules: fixed build time from
//! `WriteOptions::mtime`, root ownership, sorted file order, and the
//! CPIO payload built through `omnizip-cpio` (hex newc, aligned).
//! Compression: gzip (default) / bzip2 / xz / zstd / none.
#![forbid(unsafe_code)]

use crate::{tags, types, HEADER_MAGIC, HEADER_SIGNED_TYPE, LEAD_MAGIC, LEAD_SIZE};
use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveError, ArchiveWriter, EntryKind, NewEntry};
use std::collections::BTreeMap;

/// Payload compression selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadCompression {
    Gzip,
    Bzip2,
    Xz,
    Zstd,
    None,
}

impl PayloadCompression {
    #[must_use]
    pub const fn name(self) -> Option<&'static str> {
        match self {
            Self::Gzip => Some("gzip"),
            Self::Bzip2 => Some("bzip2"),
            Self::Xz => Some("xz"),
            Self::Zstd => Some("zstd"),
            Self::None => None,
        }
    }

    #[must_use]
    pub const fn flags(self) -> Option<&'static str> {
        match self {
            Self::Gzip => Some("9"),
            Self::Bzip2 => Some("9"),
            Self::Xz => Some("9"),
            Self::Zstd => Some("19"),
            Self::None => None,
        }
    }
}

/// Architecture name → lead number (the Ruby `ARCHITECTURES` table).
#[must_use]
pub fn arch_number(arch: &str) -> u16 {
    match arch.to_ascii_lowercase().as_str() {
        "noarch" => 0,
        "i386" => 1,
        "i486" => 2,
        "i586" => 3,
        "i686" | "x86" => 4,
        "x86_64" | "amd64" => 9,
        "ppc" => 5,
        "sparc" => 6,
        "sparc64" => 7,
        "alpha" => 8,
        "ia64" => 11,
        "arm" => 12,
        "s390" => 14,
        "s390x" => 15,
        "ppc64" => 16,
        "aarch64" | "arm64" => 19,
        _ => 0,
    }
}

/// One header tag to serialize.
struct TagDef {
    id: u32,
    type_: u32,
    value: TagOut,
}

enum TagOut {
    Str(String),
    StrArray(Vec<String>),
    Int32(Vec<u32>),
    Int16(Vec<u16>),
}

/// Builds a deterministic RPM in memory.
pub struct RpmWriter {
    name: String,
    version: String,
    release: String,
    arch: String,
    compression: PayloadCompression,
    summary: Option<String>,
    description: Option<String>,
    license: Option<String>,
    url: Option<String>,
    files: Vec<(NewEntry, Vec<u8>)>,
    finished: bool,
}

impl RpmWriter {
    #[must_use]
    pub fn new(name: &str, version: &str, release: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            release: release.to_string(),
            arch: "noarch".into(),
            compression: PayloadCompression::Gzip,
            summary: None,
            description: None,
            license: None,
            url: None,
            files: Vec::new(),
            finished: false,
        }
    }

    #[must_use]
    pub fn with_arch(mut self, arch: String) -> Self {
        self.arch = arch;
        self
    }

    #[must_use]
    pub const fn with_compression(mut self, c: PayloadCompression) -> Self {
        self.compression = c;
        self
    }

    #[must_use]
    pub fn with_summary(mut self, s: &str) -> Self {
        self.summary = Some(s.to_string());
        self
    }

    #[must_use]
    pub fn with_description(mut self, s: &str) -> Self {
        self.description = Some(s.to_string());
        self
    }

    #[must_use]
    pub fn with_license(mut self, s: &str) -> Self {
        self.license = Some(s.to_string());
        self
    }

    #[must_use]
    pub fn with_url(mut self, s: &str) -> Self {
        self.url = Some(s.to_string());
        self
    }

    /// Serialize lead + signature + main header + compressed payload.
    ///
    /// # Errors
    ///
    /// Payload compression failures.
    pub fn finish_bytes(&mut self, options: &WriteOptions) -> Result<Vec<u8>, ArchiveError> {
        if self.finished {
            return Err(ArchiveError::InvalidArchive(
                "RPM already finished".into(),
            ));
        }

        // CPIO payload through the shared crate (deterministic newc).
        let mut cpio = omnizip_cpio::CpioWriter::new()
            .with_format(omnizip_cpio::CpioFormat::Newc);
        for (entry, data) in &self.files {
            match entry.kind {
                EntryKind::Directory => cpio.add_directory(entry, options)?,
                EntryKind::Symlink(_) => cpio.add_symlink(entry, options)?,
                _ => cpio.add_file(entry, data, options)?,
            }
        }
        let raw_cpio = cpio.finish_bytes()?;
        let (payload, comp_name, comp_flags) = match self.compression {
            PayloadCompression::Gzip => (
                omnizip_archive_core::formats::gzip::compress(
                    &raw_cpio,
                    &omnizip_archive_core::formats::gzip::GzipOptions::default(),
                )?,
                "gzip",
                "9",
            ),
            PayloadCompression::Bzip2 => (
                omnizip_archive_core::formats::bzip2_file::compress(&raw_cpio, 9)?,
                "bzip2",
                "9",
            ),
            PayloadCompression::Xz => (
                omnizip_lzma::xz_compress(&raw_cpio)
                    .map_err(|e| ArchiveError::InvalidArchive(format!("xz: {e}")))?,
                "xz",
                "9",
            ),
            PayloadCompression::Zstd => (
                omnizip_zstd::compress(&raw_cpio, omnizip_zstd::ZstdLevel::Default)
                    .map_err(|e| ArchiveError::InvalidArchive(format!("zstd: {e}")))?,
                "zstd",
                "19",
            ),
            PayloadCompression::None => (raw_cpio.clone(), "", ""),
        };

        let main = self.build_main_header(options, raw_cpio.len() as u32, comp_name, comp_flags);
        let sig = build_signature_header(main.len() as u32, payload.len() as u32);
        let lead = self.build_lead();

        let mut out = Vec::with_capacity(LEAD_SIZE + sig.len() + main.len() + payload.len());
        out.extend_from_slice(&lead);
        out.extend_from_slice(&sig);
        out.extend_from_slice(&main);
        out.extend_from_slice(&payload);
        self.finished = true;
        Ok(out)
    }

    fn build_lead(&self) -> Vec<u8> {
        let mut name_field = [0u8; 66];
        let nvr = format!("{}-{}-{}", self.name, self.version, self.release);
        let n = nvr.len().min(65);
        name_field[..n].copy_from_slice(&nvr.as_bytes()[..n]);

        let mut lead = Vec::with_capacity(LEAD_SIZE);
        lead.extend_from_slice(&LEAD_MAGIC);
        lead.push(3); // major
        lead.push(0); // minor
        lead.extend_from_slice(&0u16.to_be_bytes()); // binary
        lead.extend_from_slice(&arch_number(&self.arch).to_be_bytes());
        lead.extend_from_slice(&name_field);
        lead.extend_from_slice(&1u16.to_be_bytes()); // os: linux
        lead.extend_from_slice(&HEADER_SIGNED_TYPE.to_be_bytes());
        lead.extend_from_slice(&[0u8; 16]);
        debug_assert_eq!(lead.len(), LEAD_SIZE);
        lead
    }

    fn build_main_header(
        &self,
        options: &WriteOptions,
        payload_uncompressed: u32,
        comp_name: &str,
        comp_flags: &str,
    ) -> Vec<u8> {
        // Sorted file map keeps dirindexes/basenames deterministic.
        let mut sorted: BTreeMap<&str, (&NewEntry, &Vec<u8>)> = BTreeMap::new();
        for (entry, data) in &self.files {
            sorted.insert(&entry.name, (entry, data));
        }

        let mut dirnames: Vec<String> = Vec::new();
        let mut basenames: Vec<String> = Vec::new();
        let mut dirindexes: Vec<u32> = Vec::new();
        let mut modes: Vec<u16> = Vec::new();
        let mut sizes: Vec<u32> = Vec::new();
        let mut mtimes: Vec<u32> = Vec::new();
        let mut digests: Vec<String> = Vec::new();
        let mut linktos: Vec<String> = Vec::new();
        let mut owners: Vec<String> = Vec::new();
        let mut groups: Vec<String> = Vec::new();

        for (name, (entry, data)) in &sorted {
            let (dir, base) = split_path(name);
            let idx = match dirnames.iter().position(|d| d == &dir) {
                Some(i) => i,
                None => {
                    dirnames.push(dir);
                    dirnames.len() - 1
                }
            };
            dirindexes.push(idx as u32);
            basenames.push(base);
            let type_bits = match entry.kind {
                EntryKind::Directory => 0o040_000u32,
                EntryKind::Symlink(_) => 0o120_000,
                _ => 0o100_000,
            };
            modes.push((type_bits | (entry.mode & 0o7777)) as u16);
            sizes.push(match entry.kind {
                EntryKind::Directory => 4096,
                EntryKind::Symlink(ref t) => t.len() as u32,
                _ => data.len() as u32,
            });
            mtimes.push(entry.mtime.min(u32::MAX as u64) as u32);
            digests.push(match entry.kind {
                EntryKind::Regular => omnizip_crypto::md5_hex(data),
                _ => String::new(),
            });
            linktos.push(match entry.kind {
                EntryKind::Symlink(ref t) => t.clone(),
                _ => String::new(),
            });
            owners.push("root".into());
            groups.push("root".into());
        }

        let mut defs = vec![
            TagDef { id: tags::NAME, type_: types::STRING, value: TagOut::Str(self.name.clone()) },
            TagDef { id: tags::VERSION, type_: types::STRING, value: TagOut::Str(self.version.clone()) },
            TagDef { id: tags::RELEASE, type_: types::STRING, value: TagOut::Str(self.release.clone()) },
            TagDef { id: tags::SUMMARY, type_: types::STRING, value: TagOut::Str(self.summary.clone().unwrap_or_default()) },
            TagDef { id: tags::DESCRIPTION, type_: types::STRING, value: TagOut::Str(self.description.clone().unwrap_or_default()) },
            TagDef { id: tags::BUILDTIME, type_: types::INT32, value: TagOut::Int32(vec![options.mtime.min(u32::MAX as u64) as u32]) },
            TagDef { id: tags::BUILDHOST, type_: types::STRING, value: TagOut::Str(options.host_tool.clone()) },
            TagDef { id: tags::SIZE, type_: types::INT32, value: TagOut::Int32(vec![self.files.iter().map(|(_, d)| d.len() as u32).sum()]) },
            TagDef { id: tags::LICENSE, type_: types::STRING, value: TagOut::Str(self.license.clone().unwrap_or_default()) },
            TagDef { id: tags::GROUP, type_: types::STRING, value: TagOut::Str("Unspecified".into()) },
            TagDef { id: tags::URL, type_: types::STRING, value: TagOut::Str(self.url.clone().unwrap_or_default()) },
            TagDef { id: tags::OS, type_: types::STRING, value: TagOut::Str("linux".into()) },
            TagDef { id: tags::ARCH, type_: types::STRING, value: TagOut::Str(self.arch.clone()) },
            TagDef { id: tags::FILESIZES, type_: types::INT32, value: TagOut::Int32(sizes) },
            TagDef { id: tags::FILEMODES, type_: types::INT16, value: TagOut::Int16(modes) },
            TagDef { id: tags::FILEUIDS, type_: types::INT32, value: TagOut::Int32(vec![0; basenames.len()]) },
            TagDef { id: tags::FILEGIDS, type_: types::INT32, value: TagOut::Int32(vec![0; basenames.len()]) },
            TagDef { id: tags::FILEMTIMES, type_: types::INT32, value: TagOut::Int32(mtimes) },
            TagDef { id: tags::FILEDIGESTS, type_: types::STRING_ARRAY, value: TagOut::StrArray(digests) },
            TagDef { id: tags::FILELINKTOS, type_: types::STRING_ARRAY, value: TagOut::StrArray(linktos) },
            TagDef { id: tags::FILEFLAGS, type_: types::INT32, value: TagOut::Int32(vec![0; basenames.len()]) },
            TagDef { id: tags::FILEUSERNAME, type_: types::STRING_ARRAY, value: TagOut::StrArray(owners) },
            TagDef { id: tags::FILEGROUPNAME, type_: types::STRING_ARRAY, value: TagOut::StrArray(groups) },
            TagDef { id: tags::ARCHIVESIZE, type_: types::INT32, value: TagOut::Int32(vec![payload_uncompressed]) },
            TagDef { id: tags::RPMVERSION, type_: types::STRING, value: TagOut::Str("4.16.0".into()) },
            TagDef { id: tags::DIRNAMES, type_: types::STRING_ARRAY, value: TagOut::StrArray(dirnames) },
            TagDef { id: tags::BASENAMES, type_: types::STRING_ARRAY, value: TagOut::StrArray(basenames) },
            TagDef { id: tags::DIRINDEXES, type_: types::INT32, value: TagOut::Int32(dirindexes) },
            TagDef { id: tags::PAYLOADFORMAT, type_: types::STRING, value: TagOut::Str("cpio".into()) },
        ];
        if !comp_name.is_empty() {
            defs.push(TagDef { id: tags::PAYLOADCOMPRESSOR, type_: types::STRING, value: TagOut::Str(comp_name.into()) });
            defs.push(TagDef { id: tags::PAYLOADFLAGS, type_: types::STRING, value: TagOut::Str(comp_flags.into()) });
        }
        serialize_header(&defs)
    }
}

fn split_path(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(i) => (path[..=i].to_string(), path[i + 1..].to_string()),
        None => ("/".into(), path.to_string()),
    }
}

/// Serialize one header region: header-of-header + index + blob, blob
/// padded to 8 bytes (the length field covers the padding).
fn serialize_header(defs: &[TagDef]) -> Vec<u8> {
    let mut blob = Vec::new();
    let mut index: Vec<[u8; 16]> = Vec::with_capacity(defs.len());
    for d in defs {
        let offset = blob.len() as u32;
        let count = match &d.value {
            TagOut::Str(s) => {
                blob.extend_from_slice(s.as_bytes());
                blob.push(0);
                1
            }
            TagOut::StrArray(v) => {
                for s in v {
                    blob.extend_from_slice(s.as_bytes());
                    blob.push(0);
                }
                v.len().max(1) as u32
            }
            TagOut::Int32(v) => {
                for x in v {
                    blob.extend_from_slice(&x.to_be_bytes());
                }
                v.len().max(1) as u32
            }
            TagOut::Int16(v) => {
                for x in v {
                    blob.extend_from_slice(&x.to_be_bytes());
                }
                v.len().max(1) as u32
            }
        };
        let mut e = [0u8; 16];
        e[0..4].copy_from_slice(&d.id.to_be_bytes());
        e[4..8].copy_from_slice(&d.type_.to_be_bytes());
        e[8..12].copy_from_slice(&offset.to_be_bytes());
        e[12..16].copy_from_slice(&count.to_be_bytes());
        index.push(e);
    }
    let padding = (8 - (blob.len() % 8)) % 8;
    blob.resize(blob.len() + padding, 0);

    let mut out = Vec::with_capacity(16 + index.len() * 16 + blob.len());
    out.extend_from_slice(&HEADER_MAGIC);
    out.extend_from_slice(&(index.len() as u32).to_be_bytes());
    out.extend_from_slice(&(blob.len() as u32).to_be_bytes());
    for e in &index {
        out.extend_from_slice(e);
    }
    out.extend_from_slice(&blob);
    out
}

fn build_signature_header(header_size: u32, payload_size: u32) -> Vec<u8> {
    let defs = vec![TagDef {
        id: tags::SIGSIZE,
        type_: types::INT32,
        value: TagOut::Int32(vec![header_size + payload_size]),
    }];
    let mut sig = serialize_header(&defs);
    let pad = (8 - (sig.len() % 8)) % 8;
    sig.resize(sig.len() + pad, 0);
    sig
}

impl ArchiveWriter for RpmWriter {
    fn add_file(
        &mut self,
        entry: &NewEntry,
        data: &[u8],
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.files.push((entry.clone(), data.to_vec()));
        Ok(())
    }

    fn add_directory(
        &mut self,
        entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.files.push((entry.clone(), Vec::new()));
        Ok(())
    }

    fn add_symlink(
        &mut self,
        entry: &NewEntry,
        _options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.files.push((entry.clone(), Vec::new()));
        Ok(())
    }

    fn finish(&mut self) -> Result<(), ArchiveError> {
        // Nothing to flush: `finish_bytes` does the work.
        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::RpmReader;
    use omnizip_archive_core::ArchiveReader as _;

    fn build(compression: PayloadCompression) -> Vec<u8> {
        let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
        let mut w = RpmWriter::new("hello", "1.0.0", "1")
            .with_compression(compression)
            .with_summary("hello world")
            .with_license("MIT");
        w.add_directory(&NewEntry::directory("usr/share/hello", &opts), &opts)
            .unwrap();
        w.add_file(
            &NewEntry::file("usr/share/hello/hello.txt", &opts),
            b"hello rpm\n",
            &opts,
        )
        .unwrap();
        w.finish_bytes(&opts).unwrap()
    }

    #[test]
    fn round_trip_all_compressors() {
        for c in [
            PayloadCompression::Gzip,
            PayloadCompression::Bzip2,
            PayloadCompression::Xz,
            PayloadCompression::Zstd,
            PayloadCompression::None,
        ] {
            let bytes = build(c);
            let mut r = RpmReader::from_bytes(&bytes).unwrap();
            let info = r.package_info();
            assert_eq!(info.name, "hello", "{c:?}");
            assert_eq!(info.version, "1.0.0");
            let entries = r.entries().unwrap();
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            assert!(names.contains(&"usr/share/hello/hello.txt"), "{c:?}: {names:?}");
            let idx = names.iter().position(|n| *n == "usr/share/hello/hello.txt").unwrap();
            assert_eq!(r.read_entry(idx).unwrap(), b"hello rpm\n");
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(build(PayloadCompression::Gzip), build(PayloadCompression::Gzip));
    }
}
