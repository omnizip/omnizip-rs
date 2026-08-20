fn main() {
    let data = std::fs::read("/tmp/csv-1mb.csv").unwrap();
    use omnizip_codecs::{HashChainConfig, HashChainMatchFinder, Lz77Match};
    for &hlog in &[17usize, 18] {
        let cfg = HashChainConfig {
            dict_size: (1 << 24) - 16,
            min_match: 4,
            max_chain_length: 256,
            nice_match: 96,
            hash_log: hlog as u32,
            max_match_length: 271,
            hash_bytes: 4,
        };
        let mut mf = HashChainMatchFinder::new(&data, cfg);
        for pos in 0..499_996usize {
            mf.advance();
        }
        let mut out: Vec<Lz77Match> = Vec::new();
        mf.find_candidates_into(499_992, 16, 256, &mut out);
        println!("hash_log={hlog}: {} candidates: {:?}", out.len(), &out);
        let m = mf.find_match(499_992);
        println!("  find_match: {:?}", m);
    }
}
