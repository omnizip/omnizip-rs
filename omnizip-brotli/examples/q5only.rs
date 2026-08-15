fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/csv-synthetic.csv".to_string());
    let input = std::fs::read(&path).unwrap();
    let t = std::time::Instant::now();
    let out = omnizip_brotli::from_spec_encoder::compress_with_quality(&input, 5);
    println!(
        "q5: {} bytes ({:.2}%) in {:.1}s",
        out.len(),
        out.len() as f64 * 100.0 / input.len() as f64,
        t.elapsed().as_secs_f64()
    );
    std::fs::write("/tmp/q5out.br", &out).unwrap();
}
