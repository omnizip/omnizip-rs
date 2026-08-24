use omnizip_archive_core::ArchiveReader;
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let mut r = omnizip_sevenzip::reader::SevenZipReader::open(std::path::Path::new(&p)).unwrap();
    for (i, e) in r.entries().unwrap().iter().enumerate() {
        let data = r.read_entry(i).unwrap_or_default();
        println!(
            "{}: {} bytes: {:?}",
            e.name,
            data.len(),
            String::from_utf8_lossy(&data[..data.len().min(40)])
        );
    }
}
