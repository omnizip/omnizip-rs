//! Legacy ZipCrypto read interop against the Info-ZIP CLI (task 54's
//! follow-up: WinZip-AES was covered; the traditional PKWARE cipher
//! was a read gap). Skipped when `zip` is not on PATH.

use omnizip_archive_core::ArchiveReader as _;
use omnizip_zip::ZipReader;
use std::process::Command;

fn zip_available() -> bool {
    Command::new("zip")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success() || !o.stdout.is_empty())
}

fn make_encrypted_zip(level: &str, content: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write as _;
    let dir = std::env::temp_dir().join(format!("zipcrypto-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let src = dir.join("secret.txt");
    let archive = dir.join("a.zip");
    std::fs::write(&src, content).ok()?;
    let out = Command::new("zip")
        .args(["-q", "-j", level, "-P", "swordfish"])
        .arg(&archive)
        .arg(&src)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    std::fs::read(&archive).ok()
}

#[test]
fn reads_infozip_zipcrypto_entries() {
    if !zip_available() {
        eprintln!("skipping: Info-ZIP zip not found");
        return;
    }
    let content: Vec<u8> = (0..40_000u32)
        .map(|i| (i % 251) as u8)
        .chain(b"tail text with repeats repeats repeats".iter().copied())
        .collect();
    // STORE (-0) and DEFLATE (default) entries, both under ZipCrypto.
    for level in ["-0", "-6"] {
        let Some(bytes) = make_encrypted_zip(level, &content) else {
            eprintln!("skipping {level}: zip -P failed");
            return;
        };
        let mut reader = ZipReader::from_bytes(&bytes).unwrap();
        reader.set_password("swordfish");
        let entries = reader.entries().unwrap();
        assert_eq!(entries.len(), 1, "level {level}");
        let got = reader.read_entry(0).unwrap();
        assert_eq!(got, content, "level {level} content mismatch");
    }
}

#[test]
fn zipcrypto_wrong_password_is_security_error() {
    if !zip_available() {
        eprintln!("skipping: Info-ZIP zip not found");
        return;
    }
    let Some(bytes) = make_encrypted_zip("-6", b"protected content") else {
        eprintln!("skipping: zip -P failed");
        return;
    };
    let mut reader = ZipReader::from_bytes(&bytes).unwrap();
    reader.set_password("not-the-password");
    let err = reader.read_entry(0).unwrap_err();
    assert!(
        matches!(err, omnizip_archive_core::ArchiveError::Security(_)),
        "expected Security error, got {err:?}"
    );
}

#[test]
fn zipcrypto_without_password_is_security_error() {
    if !zip_available() {
        eprintln!("skipping: Info-ZIP zip not found");
        return;
    }
    let Some(bytes) = make_encrypted_zip("-6", b"protected content") else {
        eprintln!("skipping: zip -P failed");
        return;
    };
    let mut reader = ZipReader::from_bytes(&bytes).unwrap();
    let err = reader.read_entry(0).unwrap_err();
    assert!(
        matches!(err, omnizip_archive_core::ArchiveError::Security(_)),
        "expected Security error, got {err:?}"
    );
}
