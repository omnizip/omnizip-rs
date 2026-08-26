//! 7zz interop gate — the writer's output must be listed and
//! extracted byte-exact by the reference implementation, and the
//! reader must decode 7zz-written archives (solid, LZMA2, volumes,
//! -mhe encrypted headers). Skips when 7zz is not installed.
#![forbid(unsafe_code)]

use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveReader, ArchiveWriter, NewEntry};
use omnizip_sevenzip::reader::SevenZipReader;
use omnizip_sevenzip::writer::{SevenZipMethod, SevenZipWriter};
use std::path::Path;
use std::process::Command;

fn sevenzz() -> Option<std::path::PathBuf> {
    let exe = std::env::var_os("7ZZ")
        .map(std::path::PathBuf::from)
        .or_else(which_7zz);
    exe.filter(|p| p.exists())
}

fn which_7zz() -> Option<std::path::PathBuf> {
    let out = Command::new("which").arg("7zz").output().ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!s.is_empty()).then(|| std::path::PathBuf::from(s))
    } else {
        None
    }
}

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("ozip7z-oracle-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn opts() -> WriteOptions {
    WriteOptions::deterministic().with_mtime(1_700_000_000)
}

fn build_archive(method: SevenZipMethod, solid: bool, password: Option<&str>) -> Vec<u8> {
    let mut w = SevenZipWriter::new(method).with_solid(solid);
    if let Some(pw) = password {
        w = w.with_password(pw);
    }
    w.add_directory(&NewEntry::directory("doc", &opts()), &opts())
        .unwrap();
    w.add_file(
        &NewEntry::file("doc/readme.txt", &opts()),
        b"oracle round trip\n".repeat(30).as_slice(),
        &opts(),
    )
    .unwrap();
    w.add_file(
        &NewEntry::file("doc/data.bin", &opts()),
        &[0x77; 1024],
        &opts(),
    )
    .unwrap();
    w.add_file(&NewEntry::file("doc/empty.txt", &opts()), b"", &opts())
        .unwrap();
    w.finish_bytes(&opts()).unwrap()
}

fn run_7zz(dir: &Path, args: &[&str]) -> bool {
    let Some(exe) = sevenzz() else {
        return false;
    };
    Command::new(exe)
        .args(args)
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn extract_compare(dir: &Path, archive: &str, password: Option<&str>) {
    let out = dir.join("out");
    std::fs::create_dir_all(&out).expect("mkdir");
    let mut args: Vec<String> = vec!["x".into(), "-y".into()];
    if let Some(pw) = password {
        args.push(format!("-p{pw}"));
    } else {
        args.push("-p-".into());
    }
    args.push(format!("-o{}", out.display()));
    args.push(archive.into());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    assert!(
        run_7zz(dir, &refs),
        "7zz x failed for {archive} (password: {})",
        password.unwrap_or("-")
    );
    for (name, want) in [
        (
            "doc/readme.txt",
            b"oracle round trip\n".repeat(30).as_slice(),
        ),
        ("doc/data.bin", &[0x77u8; 1024][..]),
        ("doc/empty.txt", &b""[..]),
    ] {
        let got = std::fs::read(out.join(name))
            .unwrap_or_else(|e| panic!("7zz extraction missing {name} in {archive}: {e}"));
        assert_eq!(&got, want, "{archive}: {name} bytes differ");
    }
}

fn test_archive(method: SevenZipMethod, solid: bool, password: Option<&str>) {
    if sevenzz().is_none() {
        eprintln!("skipping: 7zz not found");
        return;
    }
    let archive = format!(
        "{}-{}-{}.7z",
        format!("{method:?}").to_lowercase(),
        solid,
        password.is_some()
    );
    let tmp = TempDir::new(&archive.replace('.', "_"));
    std::fs::write(
        tmp.path().join(&archive),
        build_archive(method, solid, password),
    )
    .expect("write archive");

    let mut t_args: Vec<String> = vec!["t".into()];
    if let Some(pw) = password {
        t_args.push(format!("-p{pw}"));
    } else {
        t_args.push("-p-".into());
    }
    t_args.push(archive.clone());
    let refs: Vec<&str> = t_args.iter().map(String::as_str).collect();
    assert!(
        run_7zz(tmp.path(), &refs),
        "7zz t failed for {archive} (password: {})",
        password.unwrap_or("-")
    );
    extract_compare(tmp.path(), &archive, password);
}

#[test]
fn oracle_reads_solid_and_non_solid() {
    for solid in [false, true] {
        test_archive(SevenZipMethod::Copy, solid, None);
        test_archive(SevenZipMethod::Deflate, solid, None);
        test_archive(SevenZipMethod::Bzip2, solid, None);
        test_archive(SevenZipMethod::Lzma2, solid, None);
    }
}

#[test]
fn oracle_reads_encrypted_archives() {
    for solid in [false, true] {
        test_archive(SevenZipMethod::Copy, solid, Some("secret"));
        test_archive(SevenZipMethod::Deflate, solid, Some("secret"));
        test_archive(SevenZipMethod::Lzma2, solid, Some("secret"));
    }
}

#[test]
fn oracle_reassembles_volumes() {
    if sevenzz().is_none() {
        eprintln!("skipping: 7zz not found");
        return;
    }
    let tmp = TempDir::new("vol");
    let mut w = SevenZipWriter::new(SevenZipMethod::Copy).with_solid(true);
    w.add_file(&NewEntry::file("a.txt", &opts()), &[0x21; 5000], &opts())
        .unwrap();
    w.add_file(&NewEntry::file("b.txt", &opts()), &[0x42; 700], &opts())
        .unwrap();
    for (i, part) in w.finish_volumes(&opts(), 1024).unwrap().iter().enumerate() {
        std::fs::write(tmp.path().join(format!("multi.7z.{:03}", i + 1)), part)
            .expect("write volume");
    }
    assert!(run_7zz(tmp.path(), &["t", "-p-", "multi.7z.001"]));
    extract_compare_volumes(tmp.path());
}

fn extract_compare_volumes(dir: &Path) {
    let out = dir.join("out");
    std::fs::create_dir_all(&out).expect("mkdir");
    assert!(run_7zz(
        dir,
        &[
            "x",
            "-y",
            "-p-",
            &format!("-o{}", out.display()),
            "multi.7z.001"
        ]
    ));
    assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), vec![0x21; 5000]);
    assert_eq!(std::fs::read(out.join("b.txt")).unwrap(), vec![0x42; 700]);
}

#[test]
fn reads_7zz_written_archives() {
    if sevenzz().is_none() {
        eprintln!("skipping: 7zz not found");
        return;
    }
    let tmp = TempDir::new("r7");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("sub")).expect("mkdir");
    std::fs::write(src.join("a.txt"), b"oracle generated a\n").unwrap();
    std::fs::write(src.join("empty.txt"), b"").unwrap();
    std::fs::write(src.join("sub").join("d.bin"), vec![0xAB; 2048]).unwrap();

    // Solid LZMA2 archive.
    assert!(run_7zz(
        tmp.path(),
        &["a", "-ms=on", "-mx=1", "solid.7z", "src"]
    ));
    let mut r = SevenZipReader::open(&tmp.path().join("solid.7z")).unwrap();
    let entries = r.entries().unwrap();
    let names: Vec<(String, bool)> = entries
        .iter()
        .map(|e| {
            (
                e.name.clone(),
                e.kind == omnizip_archive_core::EntryKind::Directory,
            )
        })
        .collect();
    assert!(names.contains(&("src/empty.txt".into(), false)));
    let idx = names
        .iter()
        .position(|(n, _)| n == "src/empty.txt")
        .unwrap();
    assert_eq!(r.read_entry(idx).unwrap(), b"");
    let idx = names.iter().position(|(n, _)| n == "src/a.txt").unwrap();
    assert_eq!(r.read_entry(idx).unwrap(), b"oracle generated a\n");
    let idx = names
        .iter()
        .position(|(n, _)| n == "src/sub/d.bin")
        .unwrap();
    assert_eq!(r.read_entry(idx).unwrap(), vec![0xAB; 2048]);

    // Encrypted headers + encrypted content.
    assert!(run_7zz(
        tmp.path(),
        &["a", "-mhe=on", "-ppassword", "-mx=1", "enc.7z", "src"]
    ));
    let mut r = SevenZipReader::open_with_password(&tmp.path().join("enc.7z"), "password").unwrap();
    let entries = r.entries().unwrap();
    assert!(entries.iter().any(|e| e.name == "src/a.txt"));
    let idx = entries.iter().position(|e| e.name == "src/a.txt").unwrap();
    assert_eq!(r.read_entry(idx).unwrap(), b"oracle generated a\n");
    assert!(
        SevenZipReader::open_with_password(&tmp.path().join("enc.7z"), "wrong").is_err()
            || SevenZipReader::open(&tmp.path().join("enc.7z")).is_err()
    );
}
