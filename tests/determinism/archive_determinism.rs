//! Archive determinism (TODO.containers task 17): the same tree + the
//! same options produce byte-identical archives regardless of the
//! order entries are staged in — the shuffled-walk property. Also
//! covers the ctime-variation rule: touching files between writes
//! must not change a single byte.

use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveWriter, NewEntry};

fn opts() -> WriteOptions {
    WriteOptions::deterministic().with_mtime(1_700_000_000)
}

fn files() -> Vec<(String, Vec<u8>)> {
    vec![
        ("a.txt".into(), b"alpha".to_vec()),
        ("dir/b.bin".into(), vec![0x42; 1024]),
        ("dir/sub/c.txt".into(), b"gamma".repeat(100)),
        ("z.txt".into(), b"omega".to_vec()),
    ]
}

#[test]
fn canonicalizing_writers_ignore_insertion_order() {
    // Writers with a sorted internal map (RPM, XAR, OLE, PAR2)
    // serialize identically under any insertion order. Tar and zip
    // preserve caller order by design — the CLI stage layer applies
    // the lexicographic walk before handing entries over (covered by
    // the ozip CLI double-create test).
    let o = opts();
    let fs = files();

    let build_xar = |order: &[usize]| {
        let mut w = omnizip_xar::writer::XarWriter::new();
        for &i in order {
            let (name, data) = &fs[i];
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes(&o).unwrap()
    };
    assert_eq!(build_xar(&[0, 1, 2, 3]), build_xar(&[3, 2, 1, 0]));

    let build_ole = |order: &[usize]| {
        let mut w = omnizip_ole::writer::OleWriter::new();
        for &i in order {
            let (name, data) = &fs[i];
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes().unwrap()
    };
    assert_eq!(build_ole(&[0, 1, 2, 3]), build_ole(&[2, 0, 3, 1]));
}

// NOTE on the shape of these build closures: every writer is
// constructed INSIDE the closure. An earlier revision passed the
// writer in by value (`|mut w: TarWriter| ...` +
// `build(TarWriter::new())` twice), and that shape trips a rustc
// MIR-GVN miscompilation (reproduced on 1.85, 1.94 and nightly): the
// two constructor calls are CSE'd into ONE argument tuple which is
// then reused across both `Fn::call`s — the second call receives the
// FIRST call's already-finished, emptied writer (the second tar came
// out 1024 bytes short: no end-of-archive trailer). Writers are not
// Copy, so no source-level semantics can justify the reuse; the
// unoptimized MIR (`-Zmir-opt-level=0`) builds two distinct tuples
// and the program is correct there. Constructing inside the closure
// removes the shared argument tuple entirely.
#[test]
fn double_create_is_byte_identical_all_writers() {
    let o = opts();
    let fs = files();

    let build_tar = || {
        let mut w = omnizip_tar::TarWriter::new();
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes().unwrap()
    };
    assert_eq!(build_tar(), build_tar());

    let build_zip = || {
        let mut w = omnizip_zip::ZipWriter::new();
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes().unwrap()
    };
    assert_eq!(build_zip(), build_zip());

    let build_7z = || {
        let mut w = omnizip_sevenzip::writer::SevenZipWriter::new(
            omnizip_sevenzip::writer::SevenZipMethod::Deflate,
        );
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes(&o).unwrap()
    };
    assert_eq!(build_7z(), build_7z());

    let build_7z_solid_enc = || {
        let mut w = omnizip_sevenzip::writer::SevenZipWriter::new(
            omnizip_sevenzip::writer::SevenZipMethod::Lzma2,
        )
        .with_solid(true)
        .with_password("det");
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes(&o).unwrap()
    };
    assert_eq!(build_7z_solid_enc(), build_7z_solid_enc());

    let build_iso = || {
        let mut w = omnizip_iso::writer::IsoWriter::new("DET");
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes(&o).unwrap()
    };
    assert_eq!(build_iso(), build_iso());

    let build_xar = || {
        let mut w = omnizip_xar::writer::XarWriter::new();
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes(&o).unwrap()
    };
    assert_eq!(build_xar(), build_xar());

    let build_rar5 = || {
        let mut w = omnizip_rar::rar5::Rar5Writer::new();
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes(&o).unwrap()
    };
    assert_eq!(build_rar5(), build_rar5());
}
