use omnizip_archive_core::ArchiveReader;
fn main() {
    let mut args = std::env::args().skip(1);
    let p = args.next().unwrap();
    let password = args.next();
    let path = std::path::Path::new(&p);
    let mut r = match &password {
        Some(pw) => omnizip_sevenzip::reader::SevenZipReader::open_with_password(path, pw).unwrap(),
        None => omnizip_sevenzip::reader::SevenZipReader::open(path).unwrap(),
    };
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
