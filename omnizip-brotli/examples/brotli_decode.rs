//! Differential test driver for the omnizip-brotli decoder.
//!
//! Reads a `.br` file, decodes it via `BrotliCodec::decompress`, and
//! writes the plaintext to stdout. Used by the bash test matrix in
//! `/tmp/brotli_matrix.sh` to compare against reference `brotli` output.
//!
//! Usage: `cargo run --example brotli_decode -- <input.br> <expected_len>`

use std::io::{self, Read, Write};

use omnizip_brotli::BrotliCodec;
use omnizip_codecs::Codec;

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: {} <input.br> <expected_len>", args[0]);
        std::process::exit(2);
    }
    let path = &args[1];
    let expected_len: u32 = args[2]
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "expected_len not a u32"))?;

    let mut file = std::fs::File::open(path)?;
    let mut compressed = Vec::new();
    file.read_to_end(&mut compressed)?;

    let codec = BrotliCodec::new();
    match codec.decompress(&compressed, expected_len) {
        Ok(plaintext) => {
            io::stdout().write_all(&plaintext)?;
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("decode failed: {e}");
            std::process::exit(1);
        }
    }
}
