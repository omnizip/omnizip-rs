//! CPIO writer — port of `omnizip/formats/cpio/writer.rb` on the
//! [`ArchiveWriter`] trait. newc + CRC formats; names are NUL-terminated;
//! data blocks are padded to a 4-byte boundary; the stream is terminated
//! by a TRAILER!!! entry (filename = "TRAILER!!!").
#![forbid(unsafe_code)]

use crate::{encode_hex, pad4, CpioFormat, HEADER_SIZE, MAGIC_CRC, MAGIC_NEWC};
use omnizip_archive_core::{ArchiveError, ArchiveWriter, EntryKind, NewEntry, WriteOptions};

/// Builds an in-memory CPIO archive.
pub struct CpioWriter {
    out: Vec<u8>,
    format: CpioFormat,
    finished: bool,
}

impl CpioWriter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            format: CpioFormat::Newc,
            finished: false,
        }
    }

    #[must_use]
    pub const fn with_format(mut self, format: CpioFormat) -> Self {
        self.format = format;
        self
    }

    pub fn finish_bytes(&mut self) -> Result<Vec<u8>, ArchiveError> {
        self.finish()?;
        Ok(std::mem::take(&mut self.out))
    }
}

impl Default for CpioWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchiveWriter for CpioWriter {
    fn add_file(
        &mut self,
        entry: &NewEntry,
        data: &[u8],
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.write_entry(entry, 0o100_644, data, options)
    }

    fn add_directory(
        &mut self,
        entry: &NewEntry,
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.write_entry(entry, 0o040_755, &[], options)
    }

    fn add_symlink(
        &mut self,
        entry: &NewEntry,
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        let target = match &entry.kind {
            EntryKind::Symlink(t) => t.clone(),
            _ => {
                return Err(ArchiveError::InvalidArchive(
                    "add_symlink expects Symlink".into(),
                ));
            }
        };
        self.write_entry(entry, 0o120_777, target.as_bytes(), options)
    }

    fn finish(&mut self) -> Result<(), ArchiveError> {
        if self.finished {
            return Ok(());
        }
        // TRAILER!!! entry with a synthesized NewEntry; the writer
        // supplies options to itself via a sentinel (deterministic).
        let opts = WriteOptions::deterministic();
        let trailer = NewEntry {
            name: "TRAILER!!!".into(),
            kind: EntryKind::Regular,
            mode: 0,
            mtime: opts.mtime,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
        };
        self.write_entry(&trailer, 0, &[], &opts)?;

        // Trailer header padded to HEADER_SIZE (newc fixed width).
        let mut h = vec![0u8; HEADER_SIZE];
        h[0..6].copy_from_slice(if self.format == CpioFormat::Crc {
            MAGIC_CRC
        } else {
            MAGIC_NEWC
        });
        self.out.extend_from_slice(&h);
        self.finished = true;
        Ok(())
    }
}

impl CpioWriter {
    fn write_entry(
        &mut self,
        entry: &NewEntry,
        mode: u32,
        data: &[u8],
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        let mtime = if options.mtime == 0 {
            entry.mtime
        } else {
            options.mtime
        };
        let filesize = data.len() as u64;
        let name_bytes = entry.name.as_bytes();
        let mode_with_type = match entry.kind {
            EntryKind::Directory => mode | 0o040_000,
            EntryKind::Symlink(_) => mode | 0o120_000,
            _ => mode | 0o100_000,
        };

        let mut h = vec![0u8; HEADER_SIZE];
        let magic = if self.format == CpioFormat::Crc {
            MAGIC_CRC
        } else {
            MAGIC_NEWC
        };
        h[0..6].copy_from_slice(magic);
        h[6..14].copy_from_slice(&encode_hex(0, 8)); // ino
        h[14..22].copy_from_slice(&encode_hex(u64::from(mode_with_type), 8));
        h[22..30].copy_from_slice(&encode_hex(u64::from(entry.uid), 8));
        h[30..38].copy_from_slice(&encode_hex(u64::from(entry.gid), 8));
        h[38..46].copy_from_slice(&encode_hex(1, 8)); // nlink
        h[46..54].copy_from_slice(&encode_hex(mtime, 8));
        h[54..62].copy_from_slice(&encode_hex(filesize, 8));
        let p = 62;
        // major, minor — offset by NAME size first (per newc spec).
        // CPIO names are variable-width: 8-byte mtime + ... up to here,
        // then namesize offset is at the current `p`. We fill field-by-field.
        // The exact field order is documented in the newc spec; the Ruby
        // implementation follows it. Recreate:
        h[62..70].copy_from_slice(&encode_hex(0, 8)); // devmajor (reserved)
        h[70..78].copy_from_slice(&encode_hex(0, 8)); // devminor (reserved)
        h[78..86].copy_from_slice(&encode_hex(0, 8)); // rdevmajor
        h[86..94].copy_from_slice(&encode_hex(0, 8)); // rdevminor
        h[94..102].copy_from_slice(&encode_hex(name_bytes.len() as u64 + 1, 8));
        h[102..110].copy_from_slice(&encode_hex(0, 8)); // check (CRC mode)
        h[6..14].copy_from_slice(&encode_hex(u64::from(self.entries()) + 1, 8));
        let _ = p;

        self.out.extend_from_slice(&h);
        self.out.extend_from_slice(name_bytes);
        self.out.push(0); // NUL terminator

        // Pad header+name to a 4-byte boundary.
        let hdr_name_len = HEADER_SIZE + name_bytes.len() + 1;
        let pad = pad4(hdr_name_len);
        self.out.extend(std::iter::repeat(0u8).take(pad));

        if !data.is_empty() {
            self.out.extend_from_slice(data);
            self.out
                .extend(std::iter::repeat(0u8).take(pad4(data.len())));
        }

        if self.format == CpioFormat::Crc && !data.is_empty() {
            let crc = omnizip_archive_core::crc32(data);
            self.out.extend_from_slice(&crc.to_be_bytes());
        }
        Ok(())
    }
}

impl CpioWriter {
    fn entries(&self) -> u32 {
        // Approximate index for the ino field — the writer is consumed
        // monotonically so self.out length / HEADER_SIZE is fine.
        (self.out.len() as u32) / HEADER_SIZE as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_basic_archive() {
        let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
        let mut w = CpioWriter::new();
        w.add_directory(&NewEntry::directory("etc", &opts), &opts)
            .unwrap();
        w.add_file(
            &NewEntry::file("etc/hosts", &opts),
            b"127.0.0.1 localhost\n",
            &opts,
        )
        .unwrap();
        let bytes = w.finish_bytes().unwrap();
        assert!(bytes.starts_with(b"070701"));
        assert!(bytes.ends_with(b"\0\0\0\0"));
    }
}
