// Quick test: does the Huffman encoder now work without the safety check?
use omnizip_zstd::huffman::encoder::encode_literals;
use omnizip_zstd::literals::decode_literals_section;

fn main() {
    let input: Vec<u8> = (0..50_000)
        .map(|i| if i % 100 < 50 { (i % 26 + b'a' as i32) as u8 } else { (i % 256) as u8 })
        .collect();
    match encode_literals(&input) {
        Ok(encoded) => {
            match decode_literals_section(&encoded, None) {
                Ok(section) => {
                    if section.literals == input {
                        println!("Huffman round-trip OK: {} -> {} bytes", input.len(), encoded.len());
                    } else {
                        println!("Huffman round-trip MISMATCH: decoded {} bytes", section.literals.len());
                    }
                }
                Err(e) => println!("Decode FAIL: {}", e),
            }
        }
        Err(e) => println!("Encode FAIL: {}", e),
    }
}
