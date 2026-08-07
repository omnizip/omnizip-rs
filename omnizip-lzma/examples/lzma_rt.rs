use omnizip_lzma::decoder::Lzma1Decoder;
use omnizip_lzma::encoder::Lzma1Encoder;

fn main() {
    let input = b"abcdefgh".repeat(50);
    println!("input: {} bytes", input.len());
    let enc = Lzma1Encoder::new(3, 0, 2);
    let compressed = enc.encode(&input);
    println!("compressed: {} bytes", compressed.len());

    let mut dec = Lzma1Decoder::new(3, 0, 2, 1 << 16);
    match dec.decode(&compressed, Some(input.len() as u64), true) {
        Ok(out) => {
            if out == input {
                println!("round-trip OK!");
            } else {
                println!(
                    "mismatch: first diff at byte {}",
                    input
                        .iter()
                        .zip(out.iter())
                        .position(|(a, b)| a != b)
                        .unwrap_or(input.len())
                );
            }
        }
        Err(e) => println!("decode ERR: {:?}", e),
    }
}
