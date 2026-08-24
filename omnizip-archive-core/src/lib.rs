//! Shared archive-container layer — the container analogue of
//! `omnizip-codecs`: the `ArchiveEntry` model, `ArchiveReader` /
//! `ArchiveWriter` traits, the extraction security boundary, format
//! detection, and the deterministic-write rules.
//!
//! Ported from the Ruby reference (`omnizip/entry.rb`,
//! `archive_handler.rb`, `error.rb`, `io.rb`, `file_type.rb`,
//! `extraction/`) per TODO.containers task 01; the gzip/bzip2
//! single-file formats (task 03) live in [`formats`].

#![forbid(unsafe_code)]

/// CRC-32 (IEEE, reflected) — shared by the gzip trailer, ZIP, and
/// any other container needing the zlib polynomial.
pub use formats::gzip::crc32;

pub mod detect;
pub mod error;
pub mod formats;
pub mod security;
pub mod write_options;

use std::path::Path;

pub use error::ArchiveError;
pub use write_options::WriteOptions;

/// Entry kind, unified across formats.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Regular,
    Directory,
    /// Symbolic link with its target.
    Symlink(String),
    /// Hard link with its target.
    HardLink(String),
    /// Any other typeflag/attribute (devices, FIFOs, …).
    Other(u8),
}

/// One archive member — name, size, times, ownership, method. The
/// single narrow interface the Ruby `Omnizip::Entry` mix-in
/// established; per-format extras live in the format crates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// In-archive path (`entry_name` in Ruby).
    pub name: String,
    /// Uncompressed size in bytes, if known.
    pub size: Option<u64>,
    /// Modification time, unix seconds.
    pub mtime: Option<u64>,
    /// Permission bits.
    pub mode: Option<u32>,
    pub kind: EntryKind,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub uname: String,
    pub gname: String,
    /// Format-specific compression method id (e.g. ZIP method).
    pub method: Option<u16>,
}

impl ArchiveEntry {
    /// A regular-file entry with just a name and size.
    #[must_use]
    pub fn file(name: impl Into<String>, size: u64) -> Self {
        Self {
            name: name.into(),
            size: Some(size),
            mtime: None,
            mode: None,
            kind: EntryKind::Regular,
            uid: None,
            gid: None,
            uname: String::new(),
            gname: String::new(),
            method: None,
        }
    }

    /// A directory entry (conventionally trailing `/`).
    #[must_use]
    pub fn directory(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            size: Some(0),
            mtime: None,
            mode: None,
            kind: EntryKind::Directory,
            uid: None,
            gid: None,
            uname: String::new(),
            gname: String::new(),
            method: None,
        }
    }

    #[must_use]
    pub const fn is_directory(&self) -> bool {
        matches!(self.kind, EntryKind::Directory)
    }
}

/// Read side of a format, mirroring the Ruby handler contract
/// (`list` / `extract_to`). Entries are materialized up front (the
/// Ruby reader parses the whole central directory first); data is
/// fetched per index.
pub trait ArchiveReader {
    /// Parsed entries, in archive order.
    fn entries(&mut self) -> Result<Vec<ArchiveEntry>, ArchiveError>;

    /// Read one entry's uncompressed bytes (`entries()[index]`).
    fn read_entry(&mut self, index: usize) -> Result<Vec<u8>, ArchiveError>;

    /// Extract every entry under `output_dir`, guarded by `policy`.
    /// Security lives here — no format re-implements it.
    fn extract_to(
        &mut self,
        output_dir: &Path,
        policy: &security::SecurityPolicy,
    ) -> Result<(), ArchiveError> {
        let entries = self.entries()?;
        for (index, entry) in entries.iter().enumerate() {
            let safe = policy.validate_entry(&entry.name)?;
            let dest = output_dir.join(&safe);
            match entry.kind {
                EntryKind::Directory => {
                    std::fs::create_dir_all(&dest)
                        .map_err(|e| ArchiveError::io("create_dir", &dest, e))?;
                }
                EntryKind::Symlink(ref target) => {
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| ArchiveError::io("mkdir", parent, e))?;
                    }
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(target, &dest)
                        .map_err(|e| ArchiveError::io("symlink", &dest, e))?;
                    #[cfg(not(unix))]
                    return Err(ArchiveError::UnsupportedFeature {
                        reason: "symlink extraction requires a unix host".into(),
                    });
                }
                _ => {
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)
                            .map_err(|e| ArchiveError::io("mkdir", parent, e))?;
                    }
                    let data = self.read_entry(index)?;
                    policy.check_decompression_budget(data.len() as u64, entry)?;
                    std::fs::write(&dest, &data)
                        .map_err(|e| ArchiveError::io("write", &dest, e))?;
                }
            }
        }
        Ok(())
    }
}

/// Metadata describing an entry to write, consumed by format writers.
#[derive(Clone, Debug)]
pub struct NewEntry {
    pub name: String,
    pub kind: EntryKind,
    pub mode: u32,
    pub mtime: u64,
    pub uid: u32,
    pub gid: u32,
    pub uname: String,
    pub gname: String,
}

impl NewEntry {
    /// A regular-file entry with normalized defaults from `options`.
    #[must_use]
    pub fn file(name: impl Into<String>, options: &WriteOptions) -> Self {
        Self::new(name, EntryKind::Regular, options)
    }

    /// A directory entry (trailing `/` added by format writers as needed).
    #[must_use]
    pub fn directory(name: impl Into<String>, options: &WriteOptions) -> Self {
        Self::new(name, EntryKind::Directory, options)
    }

    /// A symlink entry.
    #[must_use]
    pub fn symlink(
        name: impl Into<String>,
        target: impl Into<String>,
        options: &WriteOptions,
    ) -> Self {
        Self::new(name, EntryKind::Symlink(target.into()), options)
    }

    #[must_use]
    pub fn new(name: impl Into<String>, kind: EntryKind, options: &WriteOptions) -> Self {
        Self {
            name: name.into(),
            mode: match kind {
                EntryKind::Directory => options.dir_mode,
                _ => options.file_mode,
            },
            kind,
            mtime: options.mtime,
            uid: options.uid,
            gid: options.gid,
            uname: options.uname.clone(),
            gname: options.gname.clone(),
        }
    }
}

/// Write side of a format (the Ruby writer contract: `add_file`,
/// `add_directory`, `add_symlink`, `add_data`, `close`).
pub trait ArchiveWriter {
    /// Append a regular-file entry.
    ///
    /// # Errors
    ///
    /// Format-specific (header overflow, IO, …).
    fn add_file(
        &mut self,
        entry: &NewEntry,
        data: &[u8],
        options: &WriteOptions,
    ) -> Result<(), ArchiveError>;

    /// Append a directory entry.
    ///
    /// # Errors
    ///
    /// Format-specific.
    fn add_directory(
        &mut self,
        entry: &NewEntry,
        options: &WriteOptions,
    ) -> Result<(), ArchiveError>;

    /// Append a symlink entry (target in `entry.kind`).
    ///
    /// # Errors
    ///
    /// Format-specific.
    fn add_symlink(&mut self, entry: &NewEntry, options: &WriteOptions)
        -> Result<(), ArchiveError>;

    /// Finish the archive (trailers, central directory, …). Consuming.
    ///
    /// # Errors
    ///
    /// Format-specific.
    fn finish(&mut self) -> Result<(), ArchiveError>;
}
