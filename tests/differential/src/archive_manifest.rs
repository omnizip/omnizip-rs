//! Archive-mode differential checks (TODO.containers task 20): our
//! writers' outputs re-read through OUR readers must produce an
//! identical canonical manifest (name, size, content hash), and the
//! cross-tool oracle tier verifies the same archives extract
//! byte-exactly with the system tools where installed.

use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveReader, ArchiveWriter, NewEntry};
use std::path::PathBuf;

fn opts() -> WriteOptions {
    WriteOptions::deterministic().with_mtime(1_700_000_000)
}

fn tree() -> Vec<(String, Vec<u8>)> {
    vec![
        (
            "docs/readme.txt".into(),
            b"manifest differential\n".repeat(40).to_vec(),
        ),
        ("docs/deep/nested.bin".into(), vec![0x5C; 5000]),
        ("top.txt".into(), b"top level".to_vec()),
    ]
}

/// Canonical manifest: `name size sha1` lines, name-sorted.
fn manifest(entries: &[(String, u64, String)]) -> String {
    let mut lines: Vec<String> = entries
        .iter()
        .map(|(n, s, h)| format!("{n} {s} {h}"))
        .collect();
    lines.sort();
    lines.join("\n")
}

fn sha1_hex(data: &[u8]) -> String {
    omnizip_crypto_sha1(data)
}

// Local sha1 to avoid a dependency edge from this test crate to
// omnizip-crypto: reuse the archive-core exposed hex via tar? Keep it
// self-contained with a tiny FNV-based digest instead — the manifest
// only needs a stable content fingerprint, not a cryptographic one.
fn omnizip_crypto_sha1(data: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    format!("{h:016x}")
}

fn collect_manifest<R: ArchiveReader>(reader: &mut R) -> String {
    let mut rows = Vec::new();
    let entries = reader.entries().unwrap();
    for (i, e) in entries.iter().enumerate() {
        let data = reader.read_entry(i).unwrap_or_default();
        rows.push((e.name.clone(), data.len() as u64, sha1_hex(&data)));
    }
    manifest(&rows)
}

fn expected_manifest() -> String {
    let mut rows: Vec<(String, u64, String)> = tree()
        .into_iter()
        .map(|(n, d)| (n, d.len() as u64, sha1_hex(&d)))
        .collect();
    rows.push(("docs/".into(), 0, sha1_hex(&[])));
    rows.push(("docs/deep/".into(), 0, sha1_hex(&[])));
    manifest(&rows)
}

#[test]
fn tar_manifest_round_trip() {
    let o = opts();
    let mut w = omnizip_tar::TarWriter::new();
    for name in ["docs", "docs/deep"] {
        w.add_directory(&NewEntry::directory(name, &o), &o).unwrap();
    }
    for (name, data) in tree() {
        w.add_file(&NewEntry::file(name.clone(), &o), &data, &o)
            .unwrap();
    }
    let bytes = w.finish_bytes().unwrap();
    let mut r = omnizip_tar::TarReader::from_bytes(&bytes).unwrap();
    assert_eq!(collect_manifest(&mut r), expected_manifest());
}

#[test]
fn zip_manifest_round_trip() {
    let o = opts();
    let mut w = omnizip_zip::ZipWriter::new();
    for name in ["docs", "docs/deep"] {
        w.add_directory(&NewEntry::directory(name, &o), &o).unwrap();
    }
    for (name, data) in tree() {
        w.add_file(&NewEntry::file(name.clone(), &o), &data, &o)
            .unwrap();
    }
    let bytes = w.finish_bytes().unwrap();
    let mut r = omnizip_zip::ZipReader::from_bytes(&bytes).unwrap();
    // Zip lists dirs with trailing slash and arrives name-sorted;
    // compare the file subset.
    let got = collect_manifest(&mut r);
    for (name, data) in tree() {
        let line = format!("{name} {} {}", data.len(), sha1_hex(&data));
        assert!(got.contains(&line), "zip missing {line} in {got}");
    }
}

#[test]
fn sevenzip_manifest_round_trip() {
    let o = opts();
    let mut w = omnizip_sevenzip::writer::SevenZipWriter::new(
        omnizip_sevenzip::writer::SevenZipMethod::Bzip2,
    );
    for (name, data) in tree() {
        w.add_file(&NewEntry::file(name.clone(), &o), &data, &o)
            .unwrap();
    }
    let bytes = w.finish_bytes(&o).unwrap();
    let mut r = omnizip_sevenzip::reader::SevenZipReader::from_bytes(&bytes).unwrap();
    // 7z listing order is files-info order; directories are not
    // synthesized as entries.
    let mut rows = Vec::new();
    let entries = r.entries().unwrap();
    for (i, e) in entries.iter().enumerate() {
        let data = r.read_entry(i).unwrap_or_default();
        rows.push((e.name.clone(), data.len() as u64, sha1_hex(&data)));
    }
    let got = manifest(&rows);
    for (name, data) in tree() {
        let line = format!("{name} {} {}", data.len(), sha1_hex(&data));
        assert!(got.contains(&line), "missing {line} in {got}");
    }
}

#[test]
fn cpio_xar_rpm_manifest_round_trips() {
    let o = opts();

    let mut w = omnizip_cpio::CpioWriter::new();
    for (name, data) in tree() {
        w.add_file(&NewEntry::file(name.clone(), &o), &data, &o)
            .unwrap();
    }
    let bytes = w.finish_bytes().unwrap();
    let mut r = omnizip_cpio::CpioReader::from_bytes(&bytes).unwrap();
    let mut rows = Vec::new();
    let entries = r.entries().unwrap();
    for (i, e) in entries.iter().enumerate() {
        let data = r.read_entry(i).unwrap_or_default();
        rows.push((e.name.clone(), data.len() as u64, sha1_hex(&data)));
    }
    let got = manifest(&rows);
    for (name, data) in tree() {
        assert!(got.contains(&format!("{name} {} {}", data.len(), sha1_hex(&data))));
    }

    let mut w = omnizip_xar::writer::XarWriter::new();
    for (name, data) in tree() {
        w.add_file(&NewEntry::file(name.clone(), &o), &data, &o)
            .unwrap();
    }
    let bytes = w.finish_bytes(&o).unwrap();
    let mut r = omnizip_xar::reader::XarReader::from_bytes(&bytes).unwrap();
    let mut rows = Vec::new();
    let entries = r.entries().unwrap();
    for (i, e) in entries.iter().enumerate() {
        let data = r.read_entry(i).unwrap_or_default();
        rows.push((e.name.clone(), data.len() as u64, sha1_hex(&data)));
    }
    let got = manifest(&rows);
    for (name, data) in tree() {
        assert!(got.contains(&format!("{name} {} {}", data.len(), sha1_hex(&data))));
    }

    let mut w = omnizip_rpm::writer::RpmWriter::new("diff", "1.0", "1");
    for (name, data) in tree() {
        w.add_file(&NewEntry::file(name.clone(), &o), &data, &o)
            .unwrap();
    }
    let bytes = w.finish_bytes(&o).unwrap();
    let mut r = omnizip_rpm::reader::RpmReader::from_bytes(&bytes).unwrap();
    let mut rows = Vec::new();
    let entries = r.entries().unwrap();
    for (i, e) in entries.iter().enumerate() {
        let data = r.read_entry(i).unwrap_or_default();
        rows.push((e.name.clone(), data.len() as u64, sha1_hex(&data)));
    }
    let got = manifest(&rows);
    for (name, data) in tree() {
        assert!(got.contains(&format!("{name} {} {}", data.len(), sha1_hex(&data))));
    }
}

/// Cross-tool oracle tier: where the system tool exists, our archive
/// must extract byte-exactly (task 20's tier-2 gate).
#[test]
fn oracle_tools_extract_byte_exactly() {
    let o = opts();
    let tmp = std::env::temp_dir().join(format!("ozip-diff-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let run_tar = |archive: &PathBuf, out: &PathBuf| {
        std::process::Command::new("tar")
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(out)
            .output()
    };
    let run_unzip = |archive: &PathBuf, out: &PathBuf| {
        std::process::Command::new("unzip")
            .arg("-q")
            .arg("-o")
            .arg(archive)
            .arg("-d")
            .arg(out)
            .output()
    };

    // tar
    let mut w = omnizip_tar::TarWriter::new();
    for (name, data) in tree() {
        w.add_file(&NewEntry::file(name.clone(), &o), &data, &o)
            .unwrap();
    }
    let tar_path = tmp.join("a.tar");
    std::fs::write(&tar_path, w.finish_bytes().unwrap()).unwrap();
    let out = tmp.join("tar-x");
    std::fs::create_dir_all(&out).unwrap();
    if let Ok(o1) = run_tar(&tar_path, &out) {
        assert!(o1.status.success(), "tar rejected our archive");
        for (name, data) in tree() {
            let read = std::fs::read(out.join(&name)).unwrap();
            assert_eq!(read, data, "{name}");
        }
    }

    // zip
    let mut w = omnizip_zip::ZipWriter::new();
    for (name, data) in tree() {
        w.add_file(&NewEntry::file(name.clone(), &o), &data, &o)
            .unwrap();
    }
    let zip_path = tmp.join("a.zip");
    std::fs::write(&zip_path, w.finish_bytes().unwrap()).unwrap();
    let out = tmp.join("zip-x");
    std::fs::create_dir_all(&out).unwrap();
    if let Ok(o2) = run_unzip(&zip_path, &out) {
        assert!(o2.status.success(), "unzip rejected our archive");
        for (name, data) in tree() {
            let read = std::fs::read(out.join(&name)).unwrap();
            assert_eq!(read, data, "{name}");
        }
    }

    let _ = std::fs::remove_dir_all(&tmp);
}
