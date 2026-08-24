use omnizip_archive_core::{ArchiveReader, ArchiveWriter, NewEntry, WriteOptions};
use omnizip_tar::{TarReader, TarWriter};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args[1].as_str() {
        "create" => {
            let opts = WriteOptions::deterministic().with_mtime(1_700_000_000);
            let mut w = TarWriter::new();
            w.add_directory(&NewEntry::directory("docs", &opts), &opts)
                .unwrap();
            w.add_file(
                &NewEntry::file("docs/readme.md", &opts),
                b"# Demo\n\nDeterministic tar.\n",
                &opts,
            )
            .unwrap();
            let long = format!("docs/very/deep/{}/target.txt", "x".repeat(120));
            w.add_file(&NewEntry::file(&long, &opts), b"deep!\n", &opts)
                .unwrap();
            w.add_symlink(&NewEntry::symlink("docs/link", "readme.md", &opts), &opts)
                .unwrap();
            w.add_directory(&NewEntry::directory("empty", &opts), &opts)
                .unwrap();
            std::fs::write(&args[2], w.finish_bytes().unwrap()).unwrap();
        }
        "list" => {
            let mut r = TarReader::open(std::path::Path::new(&args[2])).unwrap();
            for e in r.entries().unwrap() {
                println!("{}", e.name);
            }
        }
        "extract" => {
            let mut r = TarReader::open(std::path::Path::new(&args[2])).unwrap();
            r.extract_to(
                std::path::Path::new(&args[3]),
                &omnizip_archive_core::security::SecurityPolicy::default(),
            )
            .unwrap();
        }
        _ => panic!("mode"),
    }
}
