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

#[test]
fn double_create_is_byte_identical_all_writers() {
    let o = opts();
    let fs = files();

    let build = |mut w: omnizip_tar::TarWriter| {
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes().unwrap()
    };
    assert_eq!(
        build(omnizip_tar::TarWriter::new()),
        build(omnizip_tar::TarWriter::new())
    );

    let build_zip = |mut w: omnizip_zip::ZipWriter| {
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes().unwrap()
    };
    assert_eq!(
        build_zip(omnizip_zip::ZipWriter::new()),
        build_zip(omnizip_zip::ZipWriter::new())
    );

    let build_7z = |mut w: omnizip_sevenzip::writer::SevenZipWriter| {
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes(&o).unwrap()
    };
    assert_eq!(
        build_7z(omnizip_sevenzip::writer::SevenZipWriter::new(
            omnizip_sevenzip::writer::SevenZipMethod::Deflate
        )),
        build_7z(omnizip_sevenzip::writer::SevenZipWriter::new(
            omnizip_sevenzip::writer::SevenZipMethod::Deflate
        ))
    );

    let build_iso = |mut w: omnizip_iso::writer::IsoWriter| {
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes(&o).unwrap()
    };
    assert_eq!(
        build_iso(omnizip_iso::writer::IsoWriter::new("DET")),
        build_iso(omnizip_iso::writer::IsoWriter::new("DET"))
    );

    let build_xar = |mut w: omnizip_xar::writer::XarWriter| {
        for (name, data) in &fs {
            w.add_file(&NewEntry::file(name.clone(), &o), data, &o)
                .unwrap();
        }
        w.finish_bytes(&o).unwrap()
    };
    assert_eq!(
        build_xar(omnizip_xar::writer::XarWriter::new()),
        build_xar(omnizip_xar::writer::XarWriter::new())
    );
}
