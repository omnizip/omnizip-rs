//! Micro-bench HuffmanTable::from_lengths with realistic tree shapes.
fn main() {
    // cmd-tree-like: 704 alphabet, mixed code lengths with max 10 bits.
    let mut lengths = vec![0u8; 704];
    let mut l = 4u8;
    for (i, slot) in lengths.iter_mut().enumerate() {
        if i % 3 == 0 {
            *slot = l;
            l = (l % 9) + 2; // 2..=10
        }
    }
    // Fix Kraft sum roughly: make first symbols deep enough — the real
    // tables always have space==0; here we only care about build speed.
    let t = std::time::Instant::now();
    let n = 200_000;
    let mut sink = 0usize;
    for _ in 0..n {
        let tab = omnizip_brotli::decoder::HuffmanTable::from_lengths(&lengths);
        sink += tab.lookup_len();
    }
    let el = t.elapsed().as_secs_f64();
    println!(
        "{n} builds in {el:.3}s = {:.0}ns/build (sink={sink})",
        el / n as f64 * 1e9
    );
}
