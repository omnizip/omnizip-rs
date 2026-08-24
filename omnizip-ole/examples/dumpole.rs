use omnizip_archive_core::{ArchiveWriter, NewEntry, WriteOptions};
fn main() {
    let out = std::env::args().nth(1).unwrap();
    let opts = WriteOptions::deterministic();
    let mut w = omnizip_ole::writer::OleWriter::new();
    w.add_file(
        &NewEntry::file("storage/big.bin", &opts),
        &[0xABu8; 8192].as_slice(),
        &opts,
    )
    .unwrap();
    w.add_file(
        &NewEntry::file("storage/small.txt", &opts),
        b"tiny ole stream".as_slice(),
        &opts,
    )
    .unwrap();
    w.add_file(
        &NewEntry::file("root.txt", &opts),
        b"at root".as_slice(),
        &opts,
    )
    .unwrap();
    std::fs::write(&out, w.finish_bytes().unwrap()).unwrap();
}
