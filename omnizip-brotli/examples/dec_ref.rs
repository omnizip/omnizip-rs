//! Decode any brotli file (ours or reference) and dump command stats.
//! Usage: dec_ref [path]

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/csv-100kb-q5.br".to_string());
    let data = std::fs::read(&path).expect("read");
    match omnizip_brotli::decoder::decode(&data) {
        Ok(d) => {
            println!("{path}: decoded OK: {} bytes", d.len());
            omnizip_brotli::decoder_full::_print_dec_stats(d.len());
        }
        Err(e) => {
            println!("{path}: decode error: {e}");
            omnizip_brotli::decoder_full::_print_dec_stats(0);
        }
    }
}
