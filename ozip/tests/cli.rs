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

/// Task 53: a single-file codec payload is a one-entry archive —
/// `c -f <codec>` compresses the file itself, `t` lists the stripped
/// name, `x` restores the bytes; and a `.tgz` still unwraps to the
/// tar inside (the historical behavior is preserved).
#[test]
fn single_file_formats_view_as_one_entry_archives() {
    fn os(s: &str) -> &std::ffi::OsStr {
        s.as_ref()
    }
    let dir = std::env::temp_dir().join(format!("ozip-single-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let src = dir.join("notes.txt");
    std::fs::write(&src, b"hello single-file world\n").unwrap();

    let payload: &[&str] = &["gz", "bz2", "xz", "zst", "lz", "lzma"];
    for ext in payload {
        let arc = dir.join(format!("notes.txt.{ext}"));

        // c -f <codec> compresses the single file.
        let (ok, _, err) = run(&[os("c"), os("-f"), os(ext), arc.as_os_str(), src.as_os_str()]);
        assert!(ok, "c -f {ext} failed: {err}");

        // t lists the stripped entry name.
        let (ok, out, err) = run(&[os("t"), arc.as_os_str()]);
        assert!(ok, "t {ext} failed: {err}");
        assert!(out.contains("notes.txt"), "t {ext} did not list notes.txt");

        // x restores the exact bytes.
        let out_dir = dir.join(format!("out-{ext}"));
        std::fs::create_dir_all(&out_dir).unwrap();
        let (ok, _, err) = run(&[os("x"), arc.as_os_str(), os("-C"), out_dir.as_os_str()]);
        assert!(ok, "x {ext} failed: {err}");
        let restored = std::fs::read(out_dir.join("notes.txt")).expect("restored file");
        assert_eq!(restored, b"hello single-file world\n", "{ext} content");
    }

    // Directory + single-file format is a guidance error.
    let (ok, _, err) = run(&[
        os("c"),
        os("-f"),
        os("gzip"),
        dir.join("bad.gz").as_os_str(),
        dir.as_os_str(),
    ]);
    assert!(!ok, "dir input + gzip format must fail");
    assert!(err.contains("single file"), "unexpected error: {err}");

    // .tgz keeps unwrapping to the tar inside.
    let (ok, _, err) = run(&[os("c"), dir.join("arch.tgz").as_os_str(), src.as_os_str()]);
    assert!(ok, "tgz create failed: {err}");
    let (ok, out, err) = run(&[os("t"), dir.join("arch.tgz").as_os_str()]);
    assert!(ok, "tgz t failed: {err}");
    assert!(out.contains("notes.txt"), "tgz t lost the tar listing");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Task 55: `ozip verify` — green on every created format, red
/// (exit 1) on a corrupted zip; `ozip metadata` emits valid JSON
/// with the entry table.
#[test]
fn verify_and_metadata_commands() {
    fn os(s: &str) -> &std::ffi::OsStr {
        s.as_ref()
    }
    let dir = std::env::temp_dir().join(format!("ozip-verify-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("data.txt");
    std::fs::write(&src, b"verify me\n").unwrap();

    for ext in ["zip", "tar", "tgz"] {
        let arc = dir.join(format!("arch.{ext}"));
        let (ok, _, err) = run(&[os("c"), arc.as_os_str(), src.as_os_str()]);
        assert!(ok, "create {ext}: {err}");
        let (ok, out, err) = run(&[os("verify"), arc.as_os_str()]);
        assert!(ok, "verify {ext} failed: {err}");
        assert!(out.contains("all 1 entries verified"), "{ext}: {out}");
    }

    // Corruption flips a mid-payload byte — verify must go red.
    let arc = dir.join("corrupt.zip");
    std::fs::copy(dir.join("arch.zip"), &arc).unwrap();
    let mut raw = std::fs::read(&arc).unwrap();
    let mid = raw.len() / 2;
    raw[mid] ^= 0xFF;
    std::fs::write(&arc, &raw).unwrap();
    let (ok, _, _) = run(&[os("verify"), arc.as_os_str()]);
    assert!(!ok, "verify accepted a corrupted zip");

    // metadata: parseable JSON header + entry table.
    let (ok, out, err) = run(&[os("metadata"), dir.join("arch.zip").as_os_str()]);
    assert!(ok, "metadata failed: {err}");
    let text = out;
    assert!(text.contains("\"entry_count\": 1"), "{text}");
    assert!(text.contains("\"name\": \"data.txt\""), "{text}");
    assert!(text.contains("\"kind\": \"file\""), "{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Task 61: `ozip convert` — extract-repack strategy: zip → tar.gz
/// and → 7z preserve entries and content; output is deterministic
/// across runs (byte-identical).
#[test]
fn convert_between_formats() {
    fn os(s: &str) -> &std::ffi::OsStr {
        s.as_ref()
    }
    let dir = std::env::temp_dir().join(format!("ozip-convert-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hello.txt"), b"convert me\n").unwrap();

    let (ok, _, err) = run(&[
        os("c"),
        dir.join("src.zip").as_os_str(),
        dir.join("hello.txt").as_os_str(),
    ]);
    assert!(ok, "create zip: {err}");

    for (fmt, out) in [("tar.gz", "a.tar.gz"), ("7z", "a.7z")] {
        let (ok, _, err) = run(&[
            os("convert"),
            dir.join("src.zip").as_os_str(),
            dir.join(out).as_os_str(),
            os("-f"),
            os(fmt),
        ]);
        assert!(ok, "convert to {fmt}: {err}");
        let (ok, listing, err) = run(&[os("t"), dir.join(out).as_os_str()]);
        assert!(ok, "list {out}: {err}");
        assert!(listing.contains("hello.txt"), "{out} listing: {listing}");
    }

    // Determinism: two conversions of the same source are byte-identical.
    let (ok, _, err) = run(&[
        os("convert"),
        dir.join("src.zip").as_os_str(),
        dir.join("d1.tar.gz").as_os_str(),
        os("-f"),
        os("tar.gz"),
    ]);
    assert!(ok, "{err}");
    let (ok, _, err) = run(&[
        os("convert"),
        dir.join("src.zip").as_os_str(),
        dir.join("d2.tar.gz").as_os_str(),
        os("-f"),
        os("tar.gz"),
    ]);
    assert!(ok, "{err}");
    let d1 = std::fs::read(dir.join("d1.tar.gz")).unwrap();
    let d2 = std::fs::read(dir.join("d2.tar.gz")).unwrap();
    assert_eq!(d1, d2, "convert output not deterministic");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Task 55 slice 2: `ozip parity` — create → verify OK → corrupt →
/// verify red (slice-indexed) → repair byte-exact; `ozip profile`
/// lists the shipped presets.
#[test]
fn parity_and_profile_commands() {
    fn os(s: &str) -> &std::ffi::OsStr {
        s.as_ref()
    }
    let dir = std::env::temp_dir().join(format!("ozip-parity-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("data.bin");
    let mut body = vec![0u8; 8192];
    for (i, b) in body.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    std::fs::write(&src, &body).unwrap();

    // create + verify green.
    let (ok, _, err) = run(&[
        os("parity"),
        os("create"),
        src.as_os_str(),
        os("-n"),
        os("6"),
    ]);
    assert!(ok, "parity create: {err}");
    let par2 = dir.join("data.bin.par2");
    let (ok, out, err) = run(&[
        os("parity"),
        os("verify"),
        src.as_os_str(),
        par2.as_os_str(),
    ]);
    assert!(ok, "parity verify: {err}");
    assert!(out.contains("OK"), "{out}");

    // Corrupt one slice; verify goes red with the slice index.
    let damaged = dir.join("damaged.bin");
    let mut raw = body.clone();
    raw[100] ^= 0xFF;
    std::fs::write(&damaged, &raw).unwrap();
    // par2 tracks the original name: work under that name.
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::copy(&par2, work.join("data.bin.par2")).unwrap();
    std::fs::write(work.join("data.bin"), &raw).unwrap();
    let (ok, _, _) = run(&[
        os("parity"),
        os("verify"),
        work.join("data.bin").as_os_str(),
        work.join("data.bin.par2").as_os_str(),
    ]);
    assert!(!ok, "parity verify accepted corruption");

    // Repair restores the original bytes exactly.
    let (ok, _, err) = run(&[
        os("parity"),
        os("repair"),
        work.join("data.bin").as_os_str(),
        work.join("data.bin.par2").as_os_str(),
        os("-o"),
        work.join("fixed.bin").as_os_str(),
    ]);
    assert!(ok, "parity repair: {err}");
    assert_eq!(std::fs::read(work.join("fixed.bin")).unwrap(), body);

    // profile list/show.
    let (ok, out, err) = run(&[os("profile"), os("list")]);
    assert!(ok, "profile list: {err}");
    assert!(out.contains("balanced") && out.contains("maximum"), "{out}");
    let (ok, out, err) = run(&[os("profile"), os("show"), os("maximum")]);
    assert!(ok, "profile show: {err}");
    assert!(out.contains("solid:       true"), "{out}");
    let (ok, _, _) = run(&[os("profile"), os("show"), os("nope")]);
    assert!(!ok, "unknown profile must fail");

    let _ = std::fs::remove_dir_all(&dir);
}
