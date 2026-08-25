//! Read an MSI's tables and File rows.
fn main() {
    let p = std::env::args().nth(1).unwrap();
    match omnizip_ole::msi::MsiReader::open(std::path::Path::new(&p)) {
        Ok(r) => {
            println!(
                "tables: {:?}",
                r.tables.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
            );
            for (key, comp, size, name) in r.file_rows().iter().take(5) {
                println!("file: {key} comp={comp} size={size} name={name}");
            }
        }
        Err(e) => println!("ERR: {e}"),
    }
}
