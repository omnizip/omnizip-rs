//! Extraction-security corpus (TODO.containers task 21): every threat
//! row gets a crafted malicious archive that MUST be rejected by the
//! shared SecurityPolicy at the extraction boundary — never a
//! per-format branch. The corpus generates the archives in-memory
//! (tar via omnizip-tar's writer with poisoned names, zip via a
//! raw-store writer, cpio via its writer), then asserts `extract_to`
//! fails and nothing lands outside the destination.

use omnizip_archive_core::security::SecurityPolicy;
use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveReader, ArchiveWriter, NewEntry};
use std::path::Path;

fn opts() -> WriteOptions {
    WriteOptions::deterministic().with_mtime(1_700_000_000)
}

fn poison_tar(name: &str) -> Vec<u8> {
    let o = opts();
    let mut w = omnizip_tar::TarWriter::new();
    let mut e = NewEntry::file("decoy.txt", &o);
    e.name = name.to_string();
    w.add_file(&e, b"payload", &o).unwrap();
    w.finish_bytes().unwrap()
}

fn poison_zip(name: &str) -> Vec<u8> {
    let o = opts();
    let mut w = omnizip_zip::ZipWriter::new();
    let mut e = NewEntry::file("decoy.txt", &o);
    e.name = name.to_string();
    w.add_file(&e, b"payload", &o).unwrap();
    w.finish_bytes().unwrap()
}

fn poison_cpio(name: &str) -> Vec<u8> {
    let o = opts();
    let mut w = omnizip_cpio::CpioWriter::new();
    let mut e = NewEntry::file("decoy.txt", &o);
    e.name = name.to_string();
    w.add_file(&e, b"payload", &o).unwrap();
    w.finish_bytes().unwrap()
}

fn assert_rejected_tar(bytes: &[u8], out: &Path) {
    let mut r = omnizip_tar::TarReader::from_bytes(bytes).unwrap();
    let err = r
        .extract_to(out, &SecurityPolicy::default())
        .expect_err("tar traversal must be rejected");
    assert!(
        err.to_string().contains("traversal")
            || err.to_string().contains("absolute")
            || err.to_string().contains("not allowed"),
        "wrong error: {err}"
    );
}

fn assert_rejected_zip(bytes: &[u8], out: &Path) {
    let mut r = omnizip_zip::ZipReader::from_bytes(bytes).unwrap();
    assert!(r.extract_to(out, &SecurityPolicy::default()).is_err());
}

fn assert_rejected_cpio(bytes: &[u8], out: &Path) {
    let mut r = omnizip_cpio::CpioReader::from_bytes(bytes).unwrap();
    assert!(r.extract_to(out, &SecurityPolicy::default()).is_err());
}

fn poison_rar(name: &str) -> Vec<u8> {
    let o = opts();
    let mut w = omnizip_rar::rar5::Rar5Writer::new();
    let mut e = NewEntry::file("decoy.txt", &o);
    e.name = name.to_string();
    w.add_file(&e, b"payload", &o).unwrap();
    w.finish_bytes(&o).unwrap()
}

fn assert_rejected_rar(bytes: &[u8], out: &Path) {
    let mut r = omnizip_rar::rar5::Rar5Reader::from_bytes(bytes).unwrap();
    assert!(r.extract_to(out, &SecurityPolicy::default()).is_err());
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ozip-sec-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn rejects_zip_slip_all_formats() {
    let out = temp_dir("slip");
    for name in [
        "../escape.txt",
        "good/../../escape.txt",
        "a/b/../../../escape",
    ] {
        assert_rejected_tar(&poison_tar(name), &out);
        assert_rejected_zip(&poison_zip(name), &out);
        assert_rejected_cpio(&poison_cpio(name), &out);
        assert_rejected_rar(&poison_rar(name), &out);
    }
    assert!(!out.join("escape.txt").exists());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn rejects_absolute_paths_all_formats() {
    let out = temp_dir("abs");
    for name in ["/etc/passwd-style-attack", "//double/slash"] {
        assert_rejected_tar(&poison_tar(name), &out);
        assert_rejected_zip(&poison_zip(name), &out);
        assert_rejected_cpio(&poison_cpio(name), &out);
        assert_rejected_rar(&poison_rar(name), &out);
    }
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn rejects_drive_letters_and_unc() {
    let p = SecurityPolicy::default();
    assert!(p.validate_entry("C:\\Windows\\evil").is_err());
    assert!(p.validate_entry("\\\\server\\share\\evil").is_err());
    assert!(p.validate_entry("z:/data/evil").is_err());
}

#[test]
fn rejects_names_that_reduce_to_nothing() {
    let p = SecurityPolicy::default();
    assert!(p.validate_entry(".").is_err());
    assert!(p.validate_entry("./").is_err());
    assert!(p.validate_entry("").is_err());
}

#[test]
fn bomb_budgets_fire() {
    let p = SecurityPolicy::default();
    let e = omnizip_archive_core::ArchiveEntry::file("big.bin", 0);
    // Over per-entry cap.
    assert!(p.check_decompression_budget((1 << 31) + 1, &e).is_err());
    // Within caps passes.
    assert!(p.check_decompression_budget(1024, &e).is_ok());
}

#[test]
fn opt_outs_are_explicit() {
    let p = SecurityPolicy {
        allow_traversal: true,
        ..SecurityPolicy::default()
    };
    assert!(p.validate_entry("../out").is_ok());
    let p = SecurityPolicy {
        allow_absolute_paths: true,
        ..SecurityPolicy::default()
    };
    assert!(p.validate_entry("/abs").is_ok());
}

#[test]
fn clean_archives_extract_untouched() {
    let out = temp_dir("clean");
    let bytes = poison_tar("sub/decoy.txt");
    let mut r = omnizip_tar::TarReader::from_bytes(&bytes).unwrap();
    r.extract_to(&out, &SecurityPolicy::default()).unwrap();
    assert_eq!(
        std::fs::read(out.join("sub/decoy.txt")).unwrap(),
        b"payload"
    );
    let _ = std::fs::remove_dir_all(&out);
}

fn poison_tar_symlink(name: &str, target: &str) -> Vec<u8> {
    let o = opts();
    let mut w = omnizip_tar::TarWriter::new();
    w.add_symlink(&NewEntry::symlink(name, target, &o), &o)
        .unwrap();
    w.finish_bytes().unwrap()
}

fn poison_zip_symlink(name: &str, target: &str) -> Vec<u8> {
    let o = opts();
    let mut w = omnizip_zip::ZipWriter::new();
    w.add_symlink(&NewEntry::symlink(name, target, &o), &o)
        .unwrap();
    w.finish_bytes().unwrap()
}

#[test]
fn rejects_escaping_symlink_targets() {
    let out = temp_dir("symesc");
    for target in [
        "/etc/passwd-style-attack",
        "../../outside-tree",
        "C:\\Windows\\evil",
        "\\\\server\\share\\evil",
    ] {
        let mut r =
            omnizip_tar::TarReader::from_bytes(&poison_tar_symlink("evil", target)).unwrap();
        let err = r
            .extract_to(&out, &SecurityPolicy::default())
            .expect_err("escaping symlink target must be rejected");
        assert!(err.to_string().contains("symlink"), "wrong error: {err}");
        let mut z =
            omnizip_zip::ZipReader::from_bytes(&poison_zip_symlink("evil", target)).unwrap();
        assert!(z.extract_to(&out, &SecurityPolicy::default()).is_err());
    }
    assert!(!out.join("evil").exists());
    let _ = std::fs::remove_dir_all(&out);
}

#[cfg(unix)]
#[test]
fn extracts_in_tree_symlinks() {
    let out = temp_dir("symok");
    let o = opts();
    let mut w = omnizip_tar::TarWriter::new();
    w.add_file(&NewEntry::file("data.txt", &o), b"payload", &o)
        .unwrap();
    w.add_symlink(&NewEntry::symlink("link", "data.txt", &o), &o)
        .unwrap();
    w.add_symlink(&NewEntry::symlink("sub/parent-link", "../data.txt", &o), &o)
        .unwrap();
    let bytes = w.finish_bytes().unwrap();
    let mut r = omnizip_tar::TarReader::from_bytes(&bytes).unwrap();
    r.extract_to(&out, &SecurityPolicy::default()).unwrap();
    assert_eq!(std::fs::read(out.join("link")).unwrap(), b"payload");
    assert_eq!(
        std::fs::read(out.join("sub/parent-link")).unwrap(),
        b"payload"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[cfg(unix)]
#[test]
fn rejects_writes_through_symlinks() {
    let out = temp_dir("symwrite");
    let o = opts();
    // `alias` is a valid in-tree symlink to `real`; the next entry
    // writes THROUGH it. Even a benign target must not be traversed
    // by a later entry path.
    let mut w = omnizip_tar::TarWriter::new();
    w.add_directory(&NewEntry::directory("real", &o), &o)
        .unwrap();
    w.add_symlink(&NewEntry::symlink("alias", "real", &o), &o)
        .unwrap();
    w.add_file(&NewEntry::file("alias/pivot.txt", &o), b"pwn", &o)
        .unwrap();
    let bytes = w.finish_bytes().unwrap();
    let mut r = omnizip_tar::TarReader::from_bytes(&bytes).unwrap();
    let err = r
        .extract_to(&out, &SecurityPolicy::default())
        .expect_err("write through a symlink must be rejected");
    assert!(
        err.to_string().contains("traverses a symlink"),
        "wrong error: {err}"
    );
    assert!(!out.join("alias/pivot.txt").exists());
    assert!(!out.join("real/pivot.txt").exists());
    let _ = std::fs::remove_dir_all(&out);
}

#[cfg(unix)]
#[test]
fn symlink_escape_opt_out_restores_old_behavior() {
    let out = temp_dir("symopt");
    let o = opts();
    let mut w = omnizip_tar::TarWriter::new();
    w.add_symlink(&NewEntry::symlink("evil", "../../escapee", &o), &o)
        .unwrap();
    let bytes = w.finish_bytes().unwrap();
    let mut r = omnizip_tar::TarReader::from_bytes(&bytes).unwrap();
    let permissive = SecurityPolicy {
        allow_symlink_escape: true,
        ..SecurityPolicy::default()
    };
    r.extract_to(&out, &permissive).unwrap();
    assert!(std::fs::symlink_metadata(out.join("evil")).is_ok());
    let _ = std::fs::remove_dir_all(&out);
}
