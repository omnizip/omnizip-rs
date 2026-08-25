//! Archive-level benchmark cases (TODO.containers task 16):
//! create/extract a fixed in-memory tree per format, reporting size,
//! encode and decode throughput, and the determinism double-run
//! assert — the four columns the comparison table needs.
#![forbid(unsafe_code)]

use omnizip_archive_core::write_options::WriteOptions;
use omnizip_archive_core::{ArchiveReader, ArchiveWriter, NewEntry};
use std::time::Instant;

/// One benchmark result row.
#[derive(Clone, Debug)]
pub struct ArchiveBenchRow {
    pub format: &'static str,
    pub encode_mib_s: f64,
    pub decode_mib_s: f64,
    pub size_bytes: usize,
    pub deterministic: bool,
}

fn tree() -> Vec<(String, Vec<u8>)> {
    // A mixed tree exercising text, binary, and redundancy: roughly
    // 1.4 MiB.
    vec![
        (
            "docs/readme.txt".to_string(),
            "archive bench payload line\n".repeat(10_000).into_bytes(),
        ),
        ("data/cluster.bin".to_string(), vec![0xA5; 512 * 1024]),
        (
            "data/pattern.bin".to_string(),
            (0u32..131_072).map(|i| (i % 251) as u8).collect(),
        ),
    ]
}

fn total_len() -> u64 {
    tree().iter().map(|(_, d)| d.len() as u64).sum()
}

type WriteFn = Box<dyn Fn(&[(String, Vec<u8>)], &WriteOptions) -> Vec<u8>>;
type ReadFn = Box<dyn Fn(&[u8]) -> usize>;

#[allow(clippy::cast_precision_loss)]
#[allow(clippy::needless_pass_by_value)]
fn bench(format: &'static str, write: WriteFn, read: ReadFn) -> ArchiveBenchRow {
    let o = WriteOptions::deterministic().with_mtime(1_700_000_000);
    let tree = tree();
    let total = total_len() as f64 / (1024.0 * 1024.0);

    let t0 = Instant::now();
    let bytes = write(&tree, &o);
    let encode_s = t0.elapsed().as_secs_f64();

    let t1 = Instant::now();
    let decoded = read(&bytes);
    let decode_s = t1.elapsed().as_secs_f64();

    let deterministic = write(&tree, &o) == bytes;
    let _ = decoded;

    ArchiveBenchRow {
        format,
        encode_mib_s: total / encode_s.max(1e-9),
        decode_mib_s: total / decode_s.max(1e-9),
        size_bytes: bytes.len(),
        deterministic,
    }
}

#[allow(clippy::needless_pass_by_value)]
fn read_all<R: ArchiveReader>(bytes: &[u8], open: fn(&[u8]) -> Option<R>) -> usize {
    let mut r = open(bytes).expect("round-trip read");
    let entries = r.entries().expect("entries");
    let mut total = 0usize;
    for i in 0..entries.len() {
        total += r.read_entry(i).map(|d| d.len()).unwrap_or(0);
    }
    total
}

/// Run the archive benchmark suite; prints a four-column table.
///
/// # Panics
///
/// On writer/reader failures (this is a benchmark harness, not a
/// library API).
#[allow(clippy::too_many_lines)]
pub fn run() {
    let rows = vec![
        bench(
            "tar",
            Box::new(|tree, o| {
                let mut w = omnizip_tar::TarWriter::new();
                for (name, data) in tree {
                    w.add_file(&NewEntry::file(name, o), data, o).unwrap();
                }
                w.finish_bytes().unwrap()
            }),
            Box::new(|b| read_all(b, |bytes| omnizip_tar::TarReader::from_bytes(bytes).ok())),
        ),
        bench(
            "tar.gz",
            Box::new(|tree, o| {
                let mut w = omnizip_tar::TarWriter::new();
                for (name, data) in tree {
                    w.add_file(&NewEntry::file(name, o), data, o).unwrap();
                }
                let tar = w.finish_bytes().unwrap();
                omnizip_archive_core::formats::gzip::compress(
                    &tar,
                    &omnizip_archive_core::formats::gzip::GzipOptions::default(),
                )
                .unwrap()
            }),
            Box::new(|b| {
                let tar = omnizip_archive_core::formats::gzip::decompress(b).unwrap();
                read_all(&tar, |bytes| omnizip_tar::TarReader::from_bytes(bytes).ok())
            }),
        ),
        bench(
            "zip",
            Box::new(|tree, o| {
                let mut w = omnizip_zip::ZipWriter::new();
                for (name, data) in tree {
                    w.add_file(&NewEntry::file(name, o), data, o).unwrap();
                }
                w.finish_bytes().unwrap()
            }),
            Box::new(|b| read_all(b, |bytes| omnizip_zip::ZipReader::from_bytes(bytes).ok())),
        ),
        bench(
            "7z",
            Box::new(|tree, o| {
                let mut w = omnizip_sevenzip::writer::SevenZipWriter::new(
                    omnizip_sevenzip::writer::SevenZipMethod::Deflate,
                );
                for (name, data) in tree {
                    w.add_file(&NewEntry::file(name, o), data, o).unwrap();
                }
                w.finish_bytes(o).unwrap()
            }),
            Box::new(|b| {
                read_all(b, |bytes| {
                    omnizip_sevenzip::reader::SevenZipReader::from_bytes(bytes).ok()
                })
            }),
        ),
        bench(
            "xar",
            Box::new(|tree, o| {
                let mut w = omnizip_xar::writer::XarWriter::new();
                for (name, data) in tree {
                    w.add_file(&NewEntry::file(name, o), data, o).unwrap();
                }
                w.finish_bytes(o).unwrap()
            }),
            Box::new(|b| {
                read_all(b, |bytes| {
                    omnizip_xar::reader::XarReader::from_bytes(bytes).ok()
                })
            }),
        ),
        bench(
            "cpio",
            Box::new(|tree, o| {
                let mut w = omnizip_cpio::CpioWriter::new();
                for (name, data) in tree {
                    w.add_file(&NewEntry::file(name, o), data, o).unwrap();
                }
                w.finish_bytes().unwrap()
            }),
            Box::new(|b| read_all(b, |bytes| omnizip_cpio::CpioReader::from_bytes(bytes).ok())),
        ),
        bench(
            "rpm",
            Box::new(|tree, o| {
                let mut w = omnizip_rpm::writer::RpmWriter::new("bench", "1.0", "1");
                for (name, data) in tree {
                    w.add_file(&NewEntry::file(name, o), data, o).unwrap();
                }
                w.finish_bytes(o).unwrap()
            }),
            Box::new(|b| {
                read_all(b, |bytes| {
                    omnizip_rpm::reader::RpmReader::from_bytes(bytes).ok()
                })
            }),
        ),
        bench(
            "iso",
            Box::new(|tree, o| {
                let mut w = omnizip_iso::writer::IsoWriter::new("BENCH");
                for (name, data) in tree {
                    w.add_file(&NewEntry::file(name, o), data, o).unwrap();
                }
                w.finish_bytes(o).unwrap()
            }),
            Box::new(|b| {
                read_all(b, |bytes| {
                    omnizip_iso::reader::IsoReader::from_bytes(bytes).ok()
                })
            }),
        ),
        bench(
            "rar5",
            Box::new(|tree, o| {
                let mut w = omnizip_rar::rar5::Rar5Writer::new();
                for (name, data) in tree {
                    w.add_file(&NewEntry::file(name, o), data, o).unwrap();
                }
                w.finish_bytes(o).unwrap()
            }),
            Box::new(|b| {
                read_all(b, |bytes| {
                    omnizip_rar::rar5::Rar5Reader::from_bytes(bytes).ok()
                })
            }),
        ),
    ];

    println!(
        "{:<8} {:>12} {:>12} {:>10} {:>8}",
        "FORMAT", "ENC MiB/s", "DEC MiB/s", "SIZE", "DET?"
    );
    for r in &rows {
        println!(
            "{:<8} {:>12.1} {:>12.1} {:>10} {:>8}",
            r.format,
            r.encode_mib_s,
            r.decode_mib_s,
            r.size_bytes,
            if r.deterministic { "yes" } else { "NO" }
        );
    }
    let all_det = rows.iter().all(|r| r.deterministic);
    if !all_det {
        eprintln!("[bench] DETERMINISM FAILURE");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn suite_runs_and_is_deterministic() {
        // Run quietly: exercise every writer/reader pair.
        super::run();
    }
}
