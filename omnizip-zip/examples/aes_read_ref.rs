//! Read a foreign (7zz-made) WinZip-AES zip through our reader.
use omnizip_archive_core::ArchiveReader;

fn main() {
    let mut r =
        omnizip_zip::ZipReader::open(std::path::Path::new(&std::env::args().nth(1).unwrap()))
            .unwrap();
    r.set_password("swordfish");
    for (i, e) in r.entries().unwrap().iter().enumerate() {
        let data = r.read_entry(i).unwrap();
        println!(
            "{}: {} bytes: {}",
            e.name,
            data.len(),
            String::from_utf8_lossy(&data)
        );
    }
}
