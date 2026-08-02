//! Differential test: encode with xz-utils, decode with Rust, and vice versa.

#![forbid(unsafe_code)]

use omnizip_lzma::{lzma_alone_compress, lzma_alone_decompress};
use std::io::Write;

fn main() {
    let inputs: &[&[u8]] = &[
        b"Hello",
        b"Hello\n",
        b"AAAA",
        b"AAAA\n",
        b"",
        b"X",
        b"The quick brown fox jumps over the lazy dog.",
        b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        &[0x00, 0x01, 0x02, 0x03, 0xFF, 0xFE, 0xFD],
    ];

    let mut all_pass = true;
    for (i, input) in inputs.iter().enumerate() {
        let label = format!("test_{}_{}bytes", i, input.len());

        // Rust encode -> Rust decode
        let compressed = lzma_alone_compress(input).expect("encode");
        let decompressed = lzma_alone_decompress(&compressed).expect("decode");
        if decompressed.as_slice() != *input {
            eprintln!("FAIL (Rust round-trip): {label}");
            all_pass = false;
            continue;
        }

        // Write the Rust-encoded file for xz-utils to decode
        let rust_file = std::env::temp_dir().join(format!("{label}.rust.lzma"));
        let mut f = std::fs::File::create(&rust_file).expect("create file");
        f.write_all(&compressed).expect("write file");
        drop(f);

        // xz-utils decodes the Rust output
        let xz_decoded = std::process::Command::new("lzma")
            .arg("-d")
            .arg("-c")
            .arg(&rust_file)
            .output()
            .expect("run lzma");
        if !xz_decoded.status.success() {
            eprintln!(
                "FAIL (xz decode of Rust output): {label}: {}",
                String::from_utf8_lossy(&xz_decoded.stderr)
            );
            all_pass = false;
            continue;
        }
        if xz_decoded.stdout != *input {
            eprintln!("FAIL (xz decoded data mismatch): {label}");
            eprintln!("  expected: {:02X?}", input);
            eprintln!("  got:      {:02X?}", xz_decoded.stdout);
            all_pass = false;
            continue;
        }

        eprintln!("PASS: {label}");
        let _ = std::fs::remove_file(&rust_file);
    }

    if all_pass {
        eprintln!("\nAll differential tests passed.");
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}
