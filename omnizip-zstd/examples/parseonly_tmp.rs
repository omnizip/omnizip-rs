use omnizip_codecs::HashChainConfig;
use omnizip_codecs::HashChainMatchFinder;
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let runs: usize = std::env::var("RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let data = std::fs::read(&p).unwrap();
    let config = HashChainConfig {
        dict_size: 1 << 21,
        min_match: 3,
        max_chain_length: 1,
        nice_match: 64,
        hash_log: 13,
        hash_bytes: 4,
        max_match_length: 4096,
    };
    let t0 = std::time::Instant::now();
    let mut total_seqs = 0usize;
    for _ in 0..runs {
        let mut mf = HashChainMatchFinder::new(&data, config.clone());
        let mut seqs = 0;
        // just iterate positions
        for i in 0..data.len().saturating_sub(4) {
            if let Some(m) = mf.find_match(i) {
                seqs += 1;
                let _ = m;
            }
        }
        total_seqs = seqs;
    }
    eprintln!(
        "{:.3}s x{} ({} finds)",
        t0.elapsed().as_secs_f64(),
        runs,
        total_seqs
    );
}
