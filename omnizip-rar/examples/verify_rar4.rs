// Scratch verification harness (not committed): decode every entry of
// a RAR4 archive and print name/len/crc; compare against unrar output.
use omnizip_archive_core::ArchiveReader;
use omnizip_rar::rar3::Rar4Reader;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let password = std::env::var("RAR_PW").ok();
    let mut total = 0usize;
    let mut failed = 0usize;
    for arg in &args[1..] {
        let path = PathBuf::from(arg);
        let data = std::fs::read(&path).expect("read");
        let reader = match &password {
            Some(pw) => Rar4Reader::from_bytes_with_password(&data, pw),
            None => Rar4Reader::from_bytes(&data),
        };
        match reader {
            Ok(mut r) => match r.entries() {
                Ok(entries) => {
                    for (i, e) in entries.iter().enumerate() {
                        match r.read_entry(i) {
                            Ok(bytes) => {
                                let crc = omnizip_archive_core::crc32(&bytes);
                                println!(
                                    "OK {} {} len={} crc={:08X}",
                                    path.display(),
                                    e.name,
                                    bytes.len(),
                                    crc
                                );
                                total += 1;
                            }
                            Err(err) => {
                                println!("ERR {} {} {:?}", path.display(), e.name, err);
                                failed += 1;
                            }
                        }
                    }
                }
                Err(err) => println!("PARSE-ERR {} {:?}", path.display(), err),
            },
            Err(err) => println!("OPEN-ERR {} {:?}", path.display(), err),
        }
    }
    eprintln!("total={} failed={}", total, failed);
}
