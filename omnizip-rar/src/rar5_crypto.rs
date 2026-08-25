//! RAR5 file encryption (version 0 = AES-256), pure Rust. Key
//! derivation follows unrar's crypt5.cpp, cross-checked against the
//! unarcrypto reference implementation: PBKDF2-HMAC-SHA256 over
//! password + 16-byte salt with 2^kdf_count iterations for the AES
//! key, plus two extended derivations ((+16 and +32 rounds... block
//! counts, i.e. iteration counts 2^n+16 / 2^n+32) for the tweaked
//! checksum key and the password check; the check folds the final
//! 32-byte derivation into 8 bytes by XOR and carries a 4-byte
//! SHA-256 prefix as its own checksum. Data areas are AES-256-CBC
//! with the 16-byte IV from the CRYPT extra record.
#![forbid(unsafe_code)]

use omnizip_archive_core::ArchiveError;

/// Parsed CRYPT extra record (type 0x01).
#[derive(Clone, Debug)]
pub struct CryptInfo {
    pub flags: u64,
    pub kdf_count: u8,
    pub salt: [u8; 16],
    pub iv: [u8; 16],
    pub check: Option<[u8; 12]>,
}

fn read_vint(data: &[u8], p: &mut usize) -> Result<u64, ArchiveError> {
    let mut value: u64 = 0;
    let mut shift = 0u32;
    loop {
        let byte = *data
            .get(*p)
            .ok_or_else(|| ArchiveError::InvalidArchive("rar5: crypt record truncated".into()))?;
        *p += 1;
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 63 {
            return Err(ArchiveError::InvalidArchive("rar5: vint too long".into()));
        }
    }
}

impl CryptInfo {
    /// Parse the record body (everything after size + type vints).
    ///
    /// # Errors
    ///
    /// [`ArchiveError`] on truncation or unsupported versions.
    pub fn parse(body: &[u8]) -> Result<Self, ArchiveError> {
        let mut p = 0usize;
        let version = read_vint(body, &mut p)?;
        if version != 0 {
            return Err(ArchiveError::UnsupportedFeature {
                reason: format!("rar5: encryption version {version}"),
            });
        }
        let flags = read_vint(body, &mut p)?;
        let kdf_count = *body
            .get(p)
            .ok_or_else(|| ArchiveError::InvalidArchive("rar5: crypt record truncated".into()))?;
        p += 1;
        if kdf_count > 24 {
            return Err(ArchiveError::InvalidArchive(
                "rar5: implausible kdf count".into(),
            ));
        }
        let salt: [u8; 16] = body
            .get(p..p + 16)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| ArchiveError::InvalidArchive("rar5: crypt salt truncated".into()))?;
        p += 16;
        let iv: [u8; 16] = body
            .get(p..p + 16)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| ArchiveError::InvalidArchive("rar5: crypt iv truncated".into()))?;
        p += 16;
        let check = if flags & 0x0001 != 0 {
            Some(
                body.get(p..p + 12)
                    .and_then(|s| s.try_into().ok())
                    .ok_or_else(|| {
                        ArchiveError::InvalidArchive("rar5: crypt check truncated".into())
                    })?,
            )
        } else {
            None
        };
        Ok(Self {
            flags,
            kdf_count,
            salt,
            iv,
            check,
        })
    }
}

/// Derived keys for one (password, salt, kdf) triple.
pub struct Rar5Keys {
    pub aes_key: [u8; 32],
    /// Key for tweaked checksums (flag 0x0002); unused for plain data.
    pub checksum_key: [u8; 32],
    /// 8-byte password check fold.
    pub psw_check: [u8; 8],
}

/// Derive all three keys for a password/salt/kdf triple.
#[must_use]
pub fn derive_keys(password: &[u8], info: &CryptInfo) -> Rar5Keys {
    derive(password, &info.salt, info.kdf_count)
}

fn derive(password: &[u8], salt: &[u8; 16], kdf_count: u8) -> Rar5Keys {
    let iterations = 1u32 << kdf_count;
    let mut aes_key = [0u8; 32];
    omnizip_crypto::pbkdf2_hmac_sha256(password, salt, iterations, &mut aes_key);
    let mut checksum_key = [0u8; 32];
    omnizip_crypto::pbkdf2_hmac_sha256(password, salt, iterations + 16, &mut checksum_key);
    let mut check_source = [0u8; 32];
    omnizip_crypto::pbkdf2_hmac_sha256(password, salt, iterations + 32, &mut check_source);
    let mut psw_check = [0u8; 8];
    for (i, b) in check_source.iter().enumerate() {
        psw_check[i % 8] ^= *b;
    }
    Rar5Keys {
        aes_key,
        checksum_key,
        psw_check,
    }
}

impl Rar5Keys {
    /// Tweak a computed CRC32 into the stored "MAC" form used when
    /// the CRYPT record carries flag 0x0002: the stored value is
    /// LE32-fold(hmac_sha256(checksum_key, LE32(crc))).
    #[must_use]
    pub fn crc_mac(&self, crc: u32) -> u32 {
        let digest = omnizip_crypto::hmac_sha256(&self.checksum_key, &crc.to_le_bytes());
        let mut mac: u32 = 0;
        for (i, b) in digest.iter().enumerate() {
            mac ^= u32::from(*b) << ((i & 3) * 8);
        }
        mac
    }

    /// Validate against the stored 12-byte check value.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Security`] on a wrong password.
    pub fn verify(&self, check: &[u8; 12]) -> Result<(), ArchiveError> {
        if self.psw_check != check[..8] {
            return Err(ArchiveError::Security("rar5: wrong password".into()));
        }
        let digest = omnizip_crypto::sha256(&self.psw_check);
        if digest[..4] != check[8..] {
            return Err(ArchiveError::Security(
                "rar5: password check checksum mismatch".into(),
            ));
        }
        Ok(())
    }
}

/// Decrypt an entry data area: AES-256-CBC over the whole packed
/// buffer. Returns the plaintext (block-aligned; the caller trims to
/// the declared sizes).
///
/// # Errors
///
/// [`ArchiveError`] on a wrong password or odd block count.
pub fn decrypt_entry(
    password: &[u8],
    info: &CryptInfo,
    packed: &[u8],
) -> Result<Vec<u8>, ArchiveError> {
    let keys = derive(password, &info.salt, info.kdf_count);
    if let Some(check) = &info.check {
        keys.verify(check)?;
    }
    if packed.len() % 16 != 0 {
        return Err(ArchiveError::InvalidArchive(
            "rar5: encrypted data not block aligned".into(),
        ));
    }
    let mut buf = packed.to_vec();
    let mut cipher = omnizip_crypto::AesCbc256Decrypt::new(&keys.aes_key, &info.iv);
    cipher.decrypt(&mut buf);
    Ok(buf)
}

/// Decrypt an encrypted-header stream (archive encryption block):
/// same KDF, AES-256-CBC with a zero IV over everything that follows
/// the encryption block.
///
/// # Errors
///
/// [`ArchiveError`] on a wrong password.
pub fn decrypt_headers(
    password: &[u8],
    info: &CryptInfo,
    stream: &[u8],
    iv: [u8; 16],
) -> Result<Vec<u8>, ArchiveError> {
    let keys = derive(password, &info.salt, info.kdf_count);
    if let Some(check) = &info.check {
        keys.verify(check)?;
    }
    let mut buf = stream.to_vec();
    buf.truncate(buf.len() - buf.len() % 16);
    let mut cipher = omnizip_crypto::AesCbc256Decrypt::new(&keys.aes_key, &iv);
    cipher.decrypt(&mut buf);
    Ok(buf)
}
