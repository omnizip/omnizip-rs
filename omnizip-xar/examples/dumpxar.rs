use omnizip_archive_core::{ArchiveWriter, NewEntry, WriteOptions};
fn main() {
    let out = std::env::args().nth(1).unwrap();
    let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
    let mut w = omnizip_xar::writer::XarWriter::new();
    w.add_directory(&NewEntry::directory("docs", &opts), &opts)
        .unwrap();
    w.add_file(
        &NewEntry::file("docs/readme.txt", &opts),
        b"xar round trip\n".repeat(30).as_slice(),
        &opts,
    )
    .unwrap();
    w.add_symlink(&NewEntry::symlink("docs/link", "readme.txt", &opts), &opts)
        .unwrap();
    std::fs::write(&out, w.finish_bytes(&opts).unwrap()).unwrap();
}
