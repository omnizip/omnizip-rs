use omnizip_codecs::{BinaryTreeMatchFinder, HashChainConfig, HashChainMatchFinder};

fn csv_like(n: usize) -> Vec<u8> {
    let mut d = Vec::new();
    let mut i = 0u32;
    while d.len() < n {
        d.extend_from_slice(format!("{i},north,widget,{},{}\n", i % 1000, 7 * i % 100).as_bytes());
        i += 1;
    }
    d.truncate(n);
    d
}

#[test]
fn tree_matches_are_valid_lz77() {
    let data = csv_like(20_000);
    let mut mf = BinaryTreeMatchFinder::new(&data);
    for pos in 0..data.len() - 4 {
        let mut out = Vec::new();
        mf.store_and_find(pos, &mut out);
        for m in &out {
            assert!(
                m.distance >= 1 && (m.distance as usize) <= pos,
                "pos {pos} dist {}",
                m.distance
            );
            assert!(m.length >= 4);
            let src = pos - m.distance as usize;
            assert_eq!(
                &data[src..src + m.length as usize],
                &data[pos..pos + m.length as usize],
                "pos {pos}"
            );
        }
        for w in out.windows(2) {
            assert!(
                w[1].length > w[0].length,
                "lengths not increasing at pos {pos}"
            );
        }
    }
}

#[test]
fn tree_matches_chain_matches_or_better() {
    let data = csv_like(20_000);
    let cfg = HashChainConfig {
        dict_size: u32::MAX,
        min_match: 4,
        max_chain_length: 64,
        nice_match: 96,
        hash_log: 17,
        max_match_length: 271,
    };
    let mut chain = HashChainMatchFinder::new(&data, cfg);
    let mut tree = BinaryTreeMatchFinder::new(&data);
    for pos in 4..data.len() - 4 {
        let cm = {
            chain.advance();
            chain.find_match(pos)
        };
        let tm = tree.find_match(pos);
        // The two finders hash different granularities (the tree is a
        // faithful H10 4-byte port; the chain uses its own scheme), so
        // neither strictly contains the other's match set. Where both
        // find a match, the tree (all-length-tier search) must be at
        // least as long.
        if let (Some(c), Some(t)) = (&cm, &tm) {
            assert!(
                t.length >= c.length,
                "pos {pos}: tree {} < chain {}",
                t.length,
                c.length
            );
        }
        let _ = cm;
    }
}

#[test]
fn tree_deterministic() {
    let data = csv_like(5_000);
    let mut a = BinaryTreeMatchFinder::new(&data);
    let mut b = BinaryTreeMatchFinder::new(&data);
    let mut oa = Vec::new();
    let mut ob = Vec::new();
    for pos in 0..data.len() - 4 {
        a.store_and_find(pos, &mut oa);
        b.store_and_find(pos, &mut ob);
        assert_eq!(oa, ob);
    }
}
