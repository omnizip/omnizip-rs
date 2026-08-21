fn main() {
    let path = std::env::args().nth(1).unwrap();
    let q: i32 = std::env::args().nth(2).unwrap().parse().unwrap();
    let reps: usize = std::env::args()
        .nth(3)
        .map(|v| v.parse().unwrap())
        .unwrap_or(1);
    let cap: usize = std::env::args()
        .nth(4)
        .map(|v| v.parse().unwrap())
        .unwrap_or(3500000);
    let input = std::fs::read(&path).unwrap();
    let n = input.len().min(cap);
    let input = &input[..n];
    let mut last = 0;
    let t = std::time::Instant::now();
    for _ in 0..reps {
        last = omnizip_brotli::from_spec_encoder::compress_with_quality(input, q).len();
    }
    let s = t.elapsed().as_secs_f64();
    eprintln!(
        "q{q}: {} -> {} in {:.3}s total ({:.3}/rep)",
        input.len(),
        last,
        s,
        s / reps as f64
    );
    if let Ok(out) = std::env::var("OMNI_OUT") {
        let enc = omnizip_brotli::from_spec_encoder::compress_with_quality(input, q);
        std::fs::write(&out, &enc).unwrap();
    }
}
