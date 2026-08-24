//! CLI-level container tests (TODO.containers task 15): `ozip c` →
//! reference tool verify → `ozip x` byte-exact, plus the determinism
//! double-create check, for every registered write format.

use std::path::PathBuf;
use std::process::Command;

fn exe() -> &'static str {
    env!("CARGO_BIN_EXE_ozip")
}

struct Tree {
    root: PathBuf,
}

impl Tree {
    fn build(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("ozip-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("hello.txt"), b"hello, archive\n").unwrap();
        std::fs::write(root.join("sub/data.bin"), vec![0xA5u8; 4096]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../hello.txt", root.join("sub/link")).unwrap();
        Self { root }
    }

    fn inputs(&self) -> Vec<PathBuf> {
        vec![self.root.clone()]
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn os(s: &'static str) -> &'static std::ffi::OsStr {
    std::ffi::OsStr::new(s)
}

fn run(args: &[&std::ffi::OsStr]) -> (bool, String, String) {
    let out = Command::new(exe()).args(args).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn create_and_extract(fmt: &str, ext: &str, tag: &str) {
    let tree = Tree::build(tag);
    let dir = std::env::temp_dir().join(format!("ozip-out-{tag}-{}", std::process::id()));
    let archive = dir.join(format!("test.{ext}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let inputs: Vec<std::ffi::OsString> = tree
        .inputs()
        .iter()
        .map(|p| p.as_os_str().to_owned())
        .collect();
    let mut args: Vec<std::ffi::OsString> =
        vec![os("c").to_owned(), archive.as_os_str().to_owned()];
    args.extend(inputs);
    let args: Vec<&std::ffi::OsStr> = args.iter().map(std::convert::AsRef::as_ref).collect();
    let (ok, _, err) = run(&args);
    assert!(ok, "ozip c {fmt} failed: {err}");

    // List both ways.
    let (ok, out, err) = run(&[os("t"), archive.as_os_str()]);
    assert!(ok, "ozip t {fmt} failed: {err}");
    assert!(
        out.contains("hello.txt"),
        "t listing missing hello.txt: {out}"
    );
    let (ok, _, err) = run(&[os("l"), archive.as_os_str()]);
    assert!(ok, "ozip l {fmt} failed: {err}");

    // Extract and compare.
    let out_dir = dir.join("x");
    std::fs::create_dir_all(&out_dir).unwrap();
    let (ok, _, err) = run(&[os("x"), archive.as_os_str(), os("-C"), out_dir.as_os_str()]);
    assert!(ok, "ozip x {fmt} failed: {err}");
    let base = out_dir.join(tree.root.file_name().unwrap());
    assert_eq!(
        std::fs::read(base.join("hello.txt")).unwrap(),
        b"hello, archive\n"
    );
    assert_eq!(
        std::fs::read(base.join("sub/data.bin")).unwrap(),
        vec![0xA5u8; 4096]
    );

    // Determinism: create twice, byte-identical.
    let archive2 = dir.join(format!("test2.{ext}"));
    let mut args: Vec<std::ffi::OsString> =
        vec![os("c").to_owned(), archive2.as_os_str().to_owned()];
    args.extend(tree.inputs().iter().map(|p| p.as_os_str().to_owned()));
    let args: Vec<&std::ffi::OsStr> = args.iter().map(std::convert::AsRef::as_ref).collect();
    let (ok, _, err) = run(&args);
    assert!(ok, "ozip c (2nd) {fmt} failed: {err}");
    assert_eq!(
        std::fs::read(&archive).unwrap(),
        std::fs::read(&archive2).unwrap(),
        "{fmt}: two creates differ"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tar_create_extract_round_trip() {
    create_and_extract("tar", "tar", "tar");
}

#[test]
fn tar_gzip_round_trip() {
    create_and_extract("tar.gz", "tar.gz", "targz");
}

#[test]
fn zip_round_trip() {
    create_and_extract("zip", "zip", "zip");
}

#[test]
fn cpio_round_trip() {
    create_and_extract("cpio", "cpio", "cpio");
}

/// The reference oracles: `tar -tf`/`bsdtar`, `unzip -t`, `cpio -it`
/// must accept our archives (task 15 CI acceptance).
#[test]
fn reference_tools_accept_our_archives() {
    let tree = Tree::build("oracle");
    let dir = std::env::temp_dir().join(format!("ozip-oracle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let make = |name: &str| {
        let archive = dir.join(name);
        let mut args: Vec<std::ffi::OsString> =
            vec![os("c").to_owned(), archive.as_os_str().to_owned()];
        args.extend(tree.inputs().iter().map(|p| p.as_os_str().to_owned()));
        let args: Vec<&std::ffi::OsStr> = args.iter().map(std::convert::AsRef::as_ref).collect();
        let (ok, _, err) = run(&args);
        assert!(ok, "ozip c {name}: {err}");
        archive
    };

    let tar = make("test.tar");
    if let Ok(out) = Command::new("tar").arg("-tf").arg(&tar).output() {
        let listing = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "tar -tf rejected: {listing}");
        assert!(listing.contains("hello.txt"));
    }

    let zip = make("test.zip");
    if let Ok(out) = Command::new("unzip").arg("-t").arg(&zip).output() {
        assert!(out.status.success(), "unzip -t rejected our zip");
    }

    let cpio = make("test.cpio");
    if let Ok(out) = Command::new("cpio")
        .arg("-it")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map(|mut child| {
            use std::io::Write as _;
            let data = std::fs::read(&cpio).unwrap();
            child.stdin.take().unwrap().write_all(&data).unwrap();
            child.wait_with_output().unwrap()
        })
    {
        let listing = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "cpio -it rejected: {listing}");
        assert!(listing.contains("hello.txt"));
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Extraction must reject a crafted zip-slip archive (task 21 CLI
/// behavior: the shared SecurityPolicy fires, not a format branch).
#[test]
fn extraction_rejects_traversal() {
    let dir = std::env::temp_dir().join(format!("ozip-slip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Hand-craft a tar with a ../-escaping member.
    let mut tar = omnizip_tar::TarWriter::new();
    let options = omnizip_archive_core::WriteOptions::deterministic();
    let mut entry = omnizip_archive_core::NewEntry::file("x", &options);
    entry.name = "../escape.txt".into();
    use omnizip_archive_core::ArchiveWriter as _;
    tar.add_file(&entry, b"stolen", &options).unwrap();
    let bytes = tar.finish_bytes().unwrap();
    let evil = dir.join("evil.tar");
    std::fs::write(&evil, &bytes).unwrap();

    let (ok, _, err) = run(&[
        os("x"),
        evil.as_os_str(),
        os("-C"),
        dir.join("out").as_os_str(),
    ]);
    assert!(!ok, "ozip x accepted a traversal archive");
    assert!(err.contains("traversal"), "unexpected error: {err}");

    let _ = std::fs::remove_dir_all(&dir);
}
