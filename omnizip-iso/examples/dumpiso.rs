use omnizip_archive_core::{ArchiveWriter, NewEntry, WriteOptions};
fn main() {
    let out = std::env::args().nth(1).unwrap();
    let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
    let mut w = omnizip_iso::writer::IsoWriter::new("TESTVOL");
    w.add_directory(&NewEntry::directory("docs", &opts), &opts).unwrap();
    w.add_file(&NewEntry::file("docs/readme.txt", &opts), b"iso round trip\n".repeat(40).as_slice(), &opts).unwrap();
    w.add_file(&NewEntry::file("hello.dat", &opts), &[0x42u8; 4096], &opts).unwrap();
    std::fs::write(&out, w.finish_bytes(&opts).unwrap()).unwrap();
}
