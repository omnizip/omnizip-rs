use omnizip_archive_core::{ArchiveWriter, NewEntry, WriteOptions};
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = &args[1];
    let m = match args[2].as_str() {
        "copy" => omnizip_sevenzip::writer::SevenZipMethod::Copy,
        "deflate" => omnizip_sevenzip::writer::SevenZipMethod::Deflate,
        _ => omnizip_sevenzip::writer::SevenZipMethod::Bzip2,
    };
    let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
    let mut w = omnizip_sevenzip::writer::SevenZipWriter::new(m);
    w.add_directory(&NewEntry::directory("doc", &opts), &opts).unwrap();
    w.add_file(&NewEntry::file("doc/readme.txt", &opts), b"seven zip round trip\n".repeat(20).as_slice(), &opts).unwrap();
    w.add_file(&NewEntry::file("doc/data.bin", &opts), &[0x77u8; 1024], &opts).unwrap();
    std::fs::write(out, w.finish_bytes(&opts).unwrap()).unwrap();
}
