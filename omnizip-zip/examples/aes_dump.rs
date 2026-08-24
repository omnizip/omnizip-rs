//! Build a WinZip-AES zip fixture for reference-tool verification.
use omnizip_archive_core::{ArchiveWriter, NewEntry, WriteOptions};

fn main() {
    let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
    let mut w = omnizip_zip::ZipWriter::new().with_password("swordfish");
    w.add_file(&NewEntry::file("secret.txt", &opts), b"top secret payload\n".repeat(20).as_slice(), &opts)
        .unwrap();
    std::fs::write(std::env::args().nth(1).unwrap(), w.finish_bytes().unwrap()).unwrap();
}
