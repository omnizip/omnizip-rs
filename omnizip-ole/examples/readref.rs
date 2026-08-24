use omnizip_archive_core::ArchiveReader;
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let mut r = omnizip_ole::reader::OleReader::open(std::path::Path::new(&p)).unwrap();
    for (p, is_dir, size) in r.stream_paths() {
        println!("{p}: dir={is_dir} size={size}");
    }
    for i in 0..r.stream_paths().len() {
        if let Ok(d) = r.read_entry(i) {
            if d.len() < 200 {
                println!("  {i}: {:?}", String::from_utf8_lossy(&d));
            }
        }
    }
}
