//! Round-trip check: decode a .br file with OUR decoder and compare
//! byte-exact against the original. Usage: rt_check [br] [orig]
fn main() {
    let br = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/q11out.br".into());
    let orig = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "/tmp/csv-synthetic.csv".into());
    let data = std::fs::read(&br).expect("read br");
    let expect = std::fs::read(&orig).expect("read orig");
    match omnizip_brotli::decoder::decode(&data) {
        Ok(d) => {
            if d == expect {
                println!("RT-OK {} bytes", d.len());
            } else {
                let first = d.iter().zip(expect.iter()).position(|(a, b)| a != b);
                println!(
                    "RT-MISMATCH len {} vs {}: first diff at {:?}",
                    d.len(),
                    expect.len(),
                    first
                );
            }
        }
        Err(e) => println!("RT-FAIL: {e}"),
    }
}
