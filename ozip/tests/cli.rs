//! CLI-level container tests: `ozip c` →
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

/// Task 61: batch convert — `SOURCE... DIR` with `-f` converts every
/// source into the directory; the single-source form keeps working.
#[test]
fn batch_convert_converts_each_source() {
    fn os(s: &str) -> &std::ffi::OsStr {
        s.as_ref()
    }
    let dir = std::env::temp_dir().join(format!("ozip-batch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["a", "b"] {
        std::fs::write(dir.join(format!("{name}.txt")), format!("{name} payload\n")).unwrap();
        let (ok, _, err) = run(&[
            os("c"),
            dir.join(format!("{name}.zip")).as_os_str(),
            dir.join(format!("{name}.txt")).as_os_str(),
        ]);
        assert!(ok, "create {name}: {err}");
    }
    let out_dir = dir.join("converted");

    let (ok, _, err) = run(&[
        os("convert"),
        os("-f"),
        os("7z"),
        dir.join("a.zip").as_os_str(),
        dir.join("b.zip").as_os_str(),
        out_dir.as_os_str(),
    ]);
    assert!(ok, "batch convert: {err}");

    for name in ["a", "b"] {
        let (ok, listing, err) = run(&[os("t"), out_dir.join(format!("{name}.7z")).as_os_str()]);
        assert!(ok, "list {name}.7z: {err}");
        assert!(
            listing.contains(&format!("{name}.txt")),
            "{name}.7z listing missing the entry: {listing}"
        );
    }

    // Single form still works: two paths only.
    let (ok, _, err) = run(&[
        os("convert"),
        dir.join("a.zip").as_os_str(),
        dir.join("s.tar.gz").as_os_str(),
        os("-f"),
        os("tar.gz"),
    ]);
    assert!(ok, "single convert: {err}");
    let (ok, listing, _) = run(&[os("t"), dir.join("s.tar.gz").as_os_str()]);
    assert!(ok);
    assert!(listing.contains("a.txt"), "{listing}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Task 58 CLI integration: `ozip c --threads N` produces
/// byte-identical zip output to the serial path.
#[test]
fn threads_flag_is_byte_identical() {
    fn os(s: &str) -> &std::ffi::OsStr {
        s.as_ref()
    }
    let dir = std::env::temp_dir().join(format!("ozip-threads-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let body: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(dir.join("big.bin"), &body).unwrap();

    let (ok, _, err) = run(&[
        os("c"),
        dir.join("s.zip").as_os_str(),
        dir.join("big.bin").as_os_str(),
    ]);
    assert!(ok, "serial: {err}");
    let (ok, _, err) = run(&[
        os("c"),
        dir.join("p.zip").as_os_str(),
        dir.join("big.bin").as_os_str(),
        os("--threads"),
        os("8"),
    ]);
    assert!(ok, "parallel: {err}");
    assert_eq!(
        std::fs::read(dir.join("s.zip")).unwrap(),
        std::fs::read(dir.join("p.zip")).unwrap(),
        "--threads changed the output bytes"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Task 53 follow-up: gzip's embedded FNAME (RFC 1952 §2.3.1.2) is the
/// authoritative entry name when present (`gzip -N` semantics); without
/// one, the filename hint is used.
#[test]
fn gzip_fname_is_the_entry_name_when_present() {
    // A .gz whose FNAME differs from the archive's own file name.
    let mut member = Vec::new();
    let opts = omnizip_archive_core::formats::gzip::GzipOptions {
        original_name: Some("ORIGINAL-NAME.txt".into()),
        ..Default::default()
    };
    let gz = omnizip_archive_core::formats::gzip::compress(b"content\n", &opts).unwrap();
    member.extend_from_slice(&gz);

    let dir = std::env::temp_dir().join(format!("ozip-fname-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let arc = dir.join("whatever.gz");
    std::fs::write(&arc, &member).unwrap();

    // t lists the embedded name, not the archive file name.
    let (ok, out, err) = run(&[os("t"), arc.as_os_str()]);
    assert!(ok, "t failed: {err}");
    assert!(out.contains("ORIGINAL-NAME.txt"), "t listed {out:?}");

    // Without FNAME the stripped filename wins.
    let plain = omnizip_archive_core::formats::gzip::compress(b"x\n", &Default::default()).unwrap();
    let arc2 = dir.join("plain.txt.gz");
    std::fs::write(&arc2, &plain).unwrap();
    let (ok, out, err) = run(&[os("t"), arc2.as_os_str()]);
    assert!(ok, "t plain failed: {err}");
    assert!(out.contains("plain.txt"), "t plain listed {out:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Task 55 milestone 3: `ozip repair` mirrors the Ruby
/// ArchiveRepairCommand — RAR-only routing, structural verification,
/// per-entry CRC decode, and a recovery-record audit. Without an
/// in-archive RS implementation (rar-proprietary), corruption fails
/// LOUDLY with par2 guidance instead of faking success.
#[test]
fn repair_reports_intact_and_fails_loud_on_corruption() {
    fn os(s: &str) -> &std::ffi::OsStr {
        s.as_ref()
    }
    let dir = std::env::temp_dir().join(format!("ozip-repair-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let src = dir.join("data.bin");
    std::fs::write(&src, (0..20_000u32).map(|i| (i % 7) as u8).collect::<Vec<u8>>()).unwrap();
    let arc = dir.join("a.rar");
    let (ok, _, err) = run(&[os("c"), os("-f"), os("rar"), arc.as_os_str(), src.as_os_str()]);
    assert!(ok, "c -f rar failed: {err}");

    // Intact archive: repair succeeds with the no-repair-needed report.
    let (ok, out, err) = run(&[os("repair"), arc.as_os_str()]);
    assert!(ok, "repair intact failed: {err}");
    assert!(out.contains("intact"), "repair said {out:?}");

    // Non-RAR routing mirrors Ruby's "not supported" behavior.
    let zip_arc = dir.join("b.zip");
    let (ok, _, err) = run(&[os("c"), zip_arc.as_os_str(), src.as_os_str()]);
    assert!(ok, "c zip failed: {err}");
    let (ok, _, err) = run(&[os("repair"), zip_arc.as_os_str()]);
    assert!(!ok, "repair accepted a zip");
    assert!(err.contains("not supported"), "repair error: {err:?}");

    // Corruption: a flipped byte in the entry payload area must fail
    // loudly (exit 1) and point at the par2 alternative.
    let mut bytes = std::fs::read(&arc).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    let corrupt = dir.join("corrupt.rar");
    std::fs::write(&corrupt, &bytes).unwrap();
    let (ok, out, err) = run(&[os("repair"), corrupt.as_os_str()]);
    assert!(!ok, "repair declared a corrupted archive intact: {out:?}");
    assert!(
        out.contains("parity create") || err.contains("unrecoverable"),
        "corruption report missing guidance: {out:?} / {err:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Task matrix item: `ozip x --include GLOB` — the Ruby
/// SelectiveExtractor's glob class (full name or path-suffix match).
#[test]
fn selective_extraction_include_glob() {
    fn os(s: &str) -> &std::ffi::OsStr {
        s.as_ref()
    }
    let dir = std::env::temp_dir().join(format!("ozip-include-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("sub")).unwrap();
    std::fs::write(src.join("alpha.txt"), b"alpha").unwrap();
    std::fs::write(src.join("beta.bin"), b"beta").unwrap();
    std::fs::write(src.join("sub").join("gamma.txt"), b"gamma").unwrap();
    let arc = dir.join("sel.zip");
    let (ok, _, err) = run(&[os("c"), arc.as_os_str(), src.as_os_str()]);
    assert!(ok, "c failed: {err}");

    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let (ok, _, err) = run(&[
        os("x"),
        arc.as_os_str(),
        os("-C"),
        out.as_os_str(),
        os("--include"),
        os("*.txt"),
    ]);
    assert!(ok, "x --include failed: {err}");
    let files: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(files.contains(&"alpha.txt".to_string()), "{files:?}");
    assert!(files.contains(&"gamma.txt".to_string()), "{files:?}");
    assert!(!files.contains(&"beta.bin".to_string()), "{files:?}");

    let _ = std::fs::remove_dir_all(&dir);
}
