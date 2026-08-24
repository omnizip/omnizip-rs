//! ZIP writer — port of `omnizip/zip/output_stream.rb` shape (local
//! header per entry, then central directory + EOCD), with ZIP64
//! extras when any field exceeds u32, and deterministic normalization
//! from [`WriteOptions`] (fixed DOS time, fixed made-by, unix modes).
#![forbid(unsafe_code)]

use crate::{
    ATTR_DIRECTORY, CENTRAL_SIG, EOCD_SIG, FLAG_UTF8, LOCAL_SIG, METHOD_BZIP2, METHOD_DEFLATE,
    METHOD_STORE, METHOD_ZSTD, UNIX_DIR_MODE, UNIX_FILE_MODE, UNIX_SYMLINK_MODE, VERSION_BZIP2,
    VERSION_DEFAULT, VERSION_ZIP64, VERSION_ZSTD, ZIP64_EOCD_SIG, ZIP64_EXTRA_TAG,
    ZIP64_LOCATOR_SIG,
};
use omnizip_archive_core::crc32;
use omnizip_archive_core::{ArchiveError, ArchiveWriter, NewEntry, WriteOptions};

/// Compression methods the writer emits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZipMethod {
    Store,
    Deflate,
    Bzip2,
    Zstd,
}

impl ZipMethod {
    const fn code(self) -> u16 {
        match self {
            Self::Store => METHOD_STORE,
            Self::Deflate => METHOD_DEFLATE,
            Self::Bzip2 => METHOD_BZIP2,
            Self::Zstd => METHOD_ZSTD,
        }
    }

    fn version_needed(self) -> u16 {
        match self {
            Self::Store | Self::Deflate => VERSION_DEFAULT,
            Self::Bzip2 => VERSION_BZIP2,
            Self::Zstd => VERSION_ZSTD,
        }
    }
}

struct WrittenEntry {
    name: String,
    method: u16,
    aes: bool,
    real_method: u16,
    flags: u16,
    version_needed: u16,
    dos_time: u16,
    dos_date: u16,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    local_offset: u64,
    external_attrs: u32,
    zip64: bool,
}

/// Builds an in-memory ZIP.
pub struct ZipWriter {
    out: Vec<u8>,
    entries: Vec<WrittenEntry>,
    method: ZipMethod,
    password: Option<String>,
    aes_version: u16,
    aes_strength: crate::aes::AesStrength,
    finished: bool,
}

impl ZipWriter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            out: Vec::new(),
            entries: Vec::new(),
            method: ZipMethod::Deflate,
            password: None,
            aes_version: 2,
            aes_strength: crate::aes::AesStrength::Aes256,
            finished: false,
        }
    }

    /// Encrypt subsequent entries with WinZip AES (task 05). AE-2
    /// (CRC zeroed, HMAC-only authentication) with AES-256 unless
    /// reconfigured. The salt is derived, not random, so archives
    /// stay byte-reproducible.
    #[must_use]
    pub fn with_password(mut self, password: &str) -> Self {
        self.password = Some(password.to_string());
        self
    }

    /// Select the compression method for subsequent entries.
    #[must_use]
    pub const fn with_method(mut self, method: ZipMethod) -> Self {
        self.method = method;
        self
    }

    /// Finish and return the archive bytes.
    ///
    /// # Errors
    ///
    /// As [`ArchiveWriter::finish`].
    pub fn finish_bytes(&mut self) -> Result<Vec<u8>, ArchiveError> {
        self.finish()?;
        Ok(std::mem::take(&mut self.out))
    }
}

impl Default for ZipWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// DOS date/time from a unix timestamp (local-independent: the Ruby
/// used local Time; determinism requires the UTC calendar).
fn dos_datetime(unix: u64) -> (u16, u16) {
    let days = unix / 86_400;
    let secs = unix % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    // The DOS epoch is 1980; earlier timestamps clamp to it.
    let years = (year - 1980).max(0) as u16;
    let date = (years << 9) | ((month as u16) << 5) | day as u16;
    let time = (((secs / 3600) as u16) << 11)
        | (((secs % 3600) / 60) as u16) << 5
        | ((secs % 60 / 2) as u16);
    (time, date)
}

/// Days→(y,m,d) (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

impl ZipWriter {
    fn write_entry(
        &mut self,
        entry: &NewEntry,
        kind_marker: u8,
        data: &[u8],
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        use omnizip_archive_core::EntryKind;
        let is_dir = kind_marker == 1;
        let is_link = matches!(entry.kind, EntryKind::Symlink(_));

        let method = if is_dir {
            METHOD_STORE
        } else {
            self.method.code()
        };
        let content_crc = if is_dir { 0 } else { crc32(data) };
        let mut stored_crc = content_crc;
        let mut stored_method = method;
        let mut version_needed_override: Option<u16> = None;
        let mut payload = if is_dir {
            Vec::new()
        } else {
            compress_with(self.method, data)?
        };
        if let Some(pw) = self.password.clone() {
            if !is_dir {
                let strength = self.aes_strength;
                let salt = crate::aes::derived_salt(
                    pw.as_bytes(),
                    entry.name.as_bytes(),
                    content_crc,
                    strength,
                );
                let wire = crate::aes::encrypt(pw.as_bytes(), &salt, strength, &payload)?;
                if self.aes_version == 2 {
                    stored_crc = 0; // AE-2: HMAC replaces the CRC
                }
                stored_method = crate::aes::METHOD_AES;
                version_needed_override = Some(crate::aes::VERSION_AES);
                payload = wire;
            }
        }

        let crc = stored_crc;
        let flags = if stored_method == crate::aes::METHOD_AES {
            FLAG_UTF8 | crate::FLAG_ENCRYPTED
        } else {
            FLAG_UTF8
        };
        let version = if is_dir {
            VERSION_DEFAULT
        } else {
            version_needed_override.unwrap_or_else(|| self.method.version_needed())
        };
        let csize = payload.len() as u64;
        let usize_ = data.len() as u64;
        let zip64 = csize >= u32::MAX as u64 || usize_ >= u32::MAX as u64;

        let mtime = if options.mtime == 0 {
            entry.mtime
        } else {
            options.mtime
        };
        let (dos_time, dos_date) = dos_datetime(mtime);

        let local_offset = self.out.len() as u64;

        // Local-header extras: ZIP64 sizes on overflow, and the
        // WinZip AES marker on encrypted entries.
        let mut extra: Vec<u8> = Vec::new();
        if zip64 {
            extra.extend_from_slice(&ZIP64_EXTRA_TAG.to_le_bytes());
            extra.extend_from_slice(&16u16.to_le_bytes());
            extra.extend_from_slice(&usize_.to_le_bytes());
            extra.extend_from_slice(&csize.to_le_bytes());
        }
        if stored_method == crate::aes::METHOD_AES {
            extra.extend_from_slice(&crate::aes::extra_bytes(
                self.aes_version,
                self.aes_strength,
                self.method.code(),
            ));
        }

        let name_bytes = entry.name.as_bytes();
        let header_len = 30 + name_bytes.len() + extra.len();
        let header_pos = self.out.len();

        let mut header = Vec::with_capacity(header_len);
        header.extend_from_slice(&LOCAL_SIG.to_le_bytes());
        header.extend_from_slice(&version.to_le_bytes());
        header.extend_from_slice(&flags.to_le_bytes());
        header.extend_from_slice(&stored_method.to_le_bytes());
        header.extend_from_slice(&dos_time.to_le_bytes());
        header.extend_from_slice(&dos_date.to_le_bytes());
        header.extend_from_slice(&crc.to_le_bytes());
        header.extend_from_slice(&(csize.min(u32::MAX as u64) as u32).to_le_bytes());
        header.extend_from_slice(&(usize_.min(u32::MAX as u64) as u32).to_le_bytes());
        header.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        header.extend_from_slice(&(extra.len() as u16).to_le_bytes());
        header.extend_from_slice(name_bytes);
        header.extend_from_slice(&extra);
        debug_assert_eq!(header.len(), header_len);
        self.out.extend_from_slice(&header);
        let _ = header_pos;
        self.out.extend_from_slice(&payload);

        let external_attrs = if is_dir {
            UNIX_DIR_MODE | ATTR_DIRECTORY
        } else if is_link {
            UNIX_SYMLINK_MODE
        } else {
            UNIX_FILE_MODE | (entry.mode << 16)
        };

        self.entries.push(WrittenEntry {
            name: entry.name.clone(),
            method: stored_method,
            aes: stored_method == crate::aes::METHOD_AES,
            real_method: self.method.code(),
            flags,
            version_needed: if zip64 {
                VERSION_ZIP64.max(version)
            } else {
                version
            },
            dos_time,
            dos_date,
            crc32: crc,
            compressed_size: csize,
            uncompressed_size: usize_,
            local_offset,
            external_attrs,
            zip64,
        });
        Ok(())
    }
}

fn compress_with(method: ZipMethod, data: &[u8]) -> Result<Vec<u8>, ArchiveError> {
    match method {
        ZipMethod::Store => Ok(data.to_vec()),
        ZipMethod::Deflate => {
            let body = omnizip_libdeflate::deflate_dynamic::deflate_dynamic_huffman(data)
                .map_err(|e| ArchiveError::InvalidArchive(format!("deflate: {e}")))?;
            Ok(body.unwrap_or_else(|| {
                omnizip_libdeflate::deflate::deflate_stored(data).unwrap_or_else(|_| data.to_vec())
            }))
        }
        ZipMethod::Bzip2 => omnizip_bzip2::compress_framed(data, 9)
            .map_err(|e| ArchiveError::InvalidArchive(format!("bzip2: {e}"))),
        ZipMethod::Zstd => omnizip_zstd::compress(data, omnizip_zstd::ZstdLevel::Default)
            .map_err(|e| ArchiveError::InvalidArchive(format!("zstd: {e}"))),
    }
}

impl ArchiveWriter for ZipWriter {
    fn add_file(
        &mut self,
        entry: &NewEntry,
        data: &[u8],
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        self.write_entry(entry, 0, data, options)
    }

    fn add_directory(
        &mut self,
        entry: &NewEntry,
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        let mut name = entry.name.clone();
        if !name.ends_with('/') {
            name.push('/');
        }
        let dir_entry = NewEntry {
            name,
            ..entry.clone()
        };
        self.write_entry(&dir_entry, 1, &[], options)
    }

    fn add_symlink(
        &mut self,
        entry: &NewEntry,
        options: &WriteOptions,
    ) -> Result<(), ArchiveError> {
        let target = match &entry.kind {
            omnizip_archive_core::EntryKind::Symlink(t) => t.clone(),
            _ => {
                return Err(ArchiveError::InvalidArchive(
                    "add_symlink expects a Symlink entry".into(),
                ));
            }
        };
        // Symlink body is the target path, stored uncompressed.
        let saved = self.method;
        self.method = ZipMethod::Store;
        let result = self.write_entry(entry, 0, target.as_bytes(), options);
        self.method = saved;
        result
    }

    fn finish(&mut self) -> Result<(), ArchiveError> {
        if self.finished {
            return Ok(());
        }
        let cd_offset = self.out.len() as u64;
        let mut cd_size = 0u64;
        let zip64_needed = cd_offset >= u32::MAX as u64
            || self.entries.len() >= u16::MAX as usize
            || self
                .entries
                .iter()
                .any(|e| e.zip64 || e.local_offset >= u32::MAX as u64);

        for e in &self.entries {
            let name = e.name.as_bytes();
            let mut extra: Vec<u8> = Vec::new();
            if e.aes {
                extra.extend_from_slice(&crate::aes::extra_bytes(
                    self.aes_version,
                    self.aes_strength,
                    e.real_method,
                ));
            }
            if e.zip64 || e.local_offset >= u32::MAX as u64 {
                extra.extend_from_slice(&ZIP64_EXTRA_TAG.to_le_bytes());
                extra.extend_from_slice(&((if e.zip64 { 24 } else { 8 }) as u16).to_le_bytes());
                if e.zip64 {
                    extra.extend_from_slice(&e.uncompressed_size.to_le_bytes());
                    extra.extend_from_slice(&e.compressed_size.to_le_bytes());
                }
                extra.extend_from_slice(&e.local_offset.to_le_bytes());
            }
            let record_len = 46 + name.len() + extra.len();
            let mut rec = Vec::with_capacity(record_len);
            rec.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
            rec.extend_from_slice(&((3 << 8) | e.version_needed).to_le_bytes()); // made by unix
            rec.extend_from_slice(&e.version_needed.to_le_bytes());
            rec.extend_from_slice(&e.flags.to_le_bytes());
            rec.extend_from_slice(&e.method.to_le_bytes());
            rec.extend_from_slice(&e.dos_time.to_le_bytes());
            rec.extend_from_slice(&e.dos_date.to_le_bytes());
            rec.extend_from_slice(&e.crc32.to_le_bytes());
            rec.extend_from_slice(&(e.compressed_size.min(u32::MAX as u64) as u32).to_le_bytes());
            rec.extend_from_slice(&(e.uncompressed_size.min(u32::MAX as u64) as u32).to_le_bytes());
            rec.extend_from_slice(&(name.len() as u16).to_le_bytes());
            rec.extend_from_slice(&(extra.len() as u16).to_le_bytes());
            rec.extend_from_slice(&0u16.to_le_bytes()); // comment len
            rec.extend_from_slice(&0u16.to_le_bytes()); // disk number
            rec.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            rec.extend_from_slice(&e.external_attrs.to_le_bytes());
            rec.extend_from_slice(&(e.local_offset.min(u32::MAX as u64) as u32).to_le_bytes());
            rec.extend_from_slice(name);
            rec.extend_from_slice(&extra);
            self.out.extend_from_slice(&rec);
            cd_size += record_len as u64;
        }

        if zip64_needed {
            let zip64_eocd_offset = self.out.len() as u64;
            let mut z = Vec::with_capacity(56);
            z.extend_from_slice(&ZIP64_EOCD_SIG.to_le_bytes());
            z.extend_from_slice(&44u64.to_le_bytes());
            z.extend_from_slice(&((3 << 8) | VERSION_ZIP64).to_le_bytes());
            z.extend_from_slice(&VERSION_ZIP64.to_le_bytes());
            z.extend_from_slice(&0u32.to_le_bytes());
            z.extend_from_slice(&0u32.to_le_bytes());
            z.extend_from_slice(&(self.entries.len() as u64).to_le_bytes());
            z.extend_from_slice(&(self.entries.len() as u64).to_le_bytes());
            z.extend_from_slice(&cd_size.to_le_bytes());
            z.extend_from_slice(&cd_offset.to_le_bytes());
            z.extend_from_slice(&ZIP64_LOCATOR_SIG.to_le_bytes());
            z.extend_from_slice(&0u32.to_le_bytes());
            z.extend_from_slice(&zip64_eocd_offset.to_le_bytes());
            z.extend_from_slice(&1u32.to_le_bytes());
            self.out.extend_from_slice(&z);
        }

        let mut eocd = Vec::with_capacity(22);
        eocd.extend_from_slice(&EOCD_SIG.to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes());
        eocd.extend_from_slice(&(self.entries.len().min(u16::MAX as usize) as u16).to_le_bytes());
        eocd.extend_from_slice(&(self.entries.len().min(u16::MAX as usize) as u16).to_le_bytes());
        eocd.extend_from_slice(&(cd_size.min(u32::MAX as u64) as u32).to_le_bytes());
        eocd.extend_from_slice(&(cd_offset.min(u32::MAX as u64) as u32).to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes());
        self.out.extend_from_slice(&eocd);

        self.finished = true;
        Ok(())
    }
}

#[allow(clippy::too_many_lines)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ZipReader;
    use omnizip_archive_core::ArchiveReader;

    fn demo(method: ZipMethod) -> Vec<u8> {
        let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
        let mut w = ZipWriter::new().with_method(method);
        w.add_directory(&NewEntry::directory("d", &opts), &opts)
            .unwrap();
        w.add_file(
            &NewEntry::file("d/f.txt", &opts),
            b"zip round trip contents\n".repeat(10).as_slice(),
            &opts,
        )
        .unwrap();
        w.add_symlink(&NewEntry::symlink("d/l", "f.txt", &opts), &opts)
            .unwrap();
        w.finish_bytes().unwrap()
    }

    #[test]
    fn round_trips_all_methods() {
        for m in [
            ZipMethod::Store,
            ZipMethod::Deflate,
            ZipMethod::Bzip2,
            ZipMethod::Zstd,
        ] {
            let bytes = demo(m);
            let mut r = ZipReader::from_bytes(&bytes).unwrap();
            let entries = r.entries().unwrap();
            assert_eq!(entries.len(), 3, "{m:?}");
            assert_eq!(entries[0].name, "d/");
            assert_eq!(
                r.read_entry(1).unwrap(),
                b"zip round trip contents\n".repeat(10)
            );
            assert_eq!(
                entries[2].kind,
                omnizip_archive_core::EntryKind::Symlink("f.txt".into())
            );
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(demo(ZipMethod::Deflate), demo(ZipMethod::Deflate));
    }

    #[test]
    fn winzip_aes_round_trip_and_wrong_password() {
        let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
        let mut w = ZipWriter::new().with_password("swordfish");
        w.add_file(&NewEntry::file("secret.txt", &opts), b"top secret payload\n".repeat(20).as_slice(), &opts)
            .unwrap();
        w.add_file(&NewEntry::file("plain.bin", &opts), &[0x5A; 512], &opts)
            .unwrap();
        let bytes = w.finish_bytes().unwrap();

        // Deterministic even encrypted: derived salts.
        let mut w2 = ZipWriter::new().with_password("swordfish");
        w2.add_file(&NewEntry::file("secret.txt", &opts), b"top secret payload\n".repeat(20).as_slice(), &opts)
            .unwrap();
        w2.add_file(&NewEntry::file("plain.bin", &opts), &[0x5A; 512], &opts)
            .unwrap();
        assert_eq!(bytes, w2.finish_bytes().unwrap());

        let mut r = ZipReader::from_bytes(&bytes).unwrap();
        r.set_password("swordfish");
        let entries = r.entries().unwrap();
        assert_eq!(entries[0].name, "secret.txt");
        assert_eq!(r.read_entry(0).unwrap(), b"top secret payload\n".repeat(20));
        assert_eq!(r.read_entry(1).unwrap(), vec![0x5A; 512]);

        // Wrong password: verification bytes, not padding.
        let mut bad = ZipReader::from_bytes(&bytes).unwrap();
        bad.set_password("wrong");
        let err = bad.read_entry(0).unwrap_err();
        assert!(err.to_string().contains("verification"), "{err}");

        // No password at all: clear error naming the entry.
        let mut none = ZipReader::from_bytes(&bytes).unwrap();
        let err = none.read_entry(0).unwrap_err();
        assert!(err.to_string().contains("supply a password"), "{err}");
    }

    #[test]
    fn dos_time_epoch() {
        assert_eq!(dos_datetime(0), (0, 0x21)); // 1980-01-01 00:00
    }
}
