//! Benchmark: omnizip-brotli from_spec_encoder vs vendored C reference.
//!
//! Compares ratio and speed across representative data types at
//! multiple quality levels.

use omnizip_brotli::{BrotliCodec, BrotliOptions};
use omnizip_codecs::{Codec, CompressionLevel};
use std::time::Instant;

fn csv_data(size_kb: usize) -> Vec<u8> {
    let row = b"id,name,city,country,population,area_code,latitude,longitude,status\n";
    let rows_per_iter = size_kb * 1024 / row.len();
    let mut data = Vec::with_capacity(size_kb * 1024);
    for i in 0..rows_per_iter {
        data.extend_from_slice(
            format!(
                "{i},user_{i},city_{},cc,{},{},{}.{},{i}\n",
                i % 1000,
                i % 1000000,
                i % 360 - 180,
                i / 1000,
                i % 1000,
            )
            .as_bytes(),
        );
    }
    data
}

fn english_text(size_kb: usize) -> Vec<u8> {
    let words = b"the quick brown fox jumps over the lazy dog and runs through the forest looking for food while the sun sets behind the mountains creating shadows that stretch across the valley floor where the river flows gently toward the sea ";
    let mut data = Vec::with_capacity(size_kb * 1024);
    while data.len() < size_kb * 1024 {
        let start = data.len() % words.len();
        data.extend_from_slice(&words[start..]);
    }
    data.truncate(size_kb * 1024);
    data
}

fn binary_data(size_kb: usize) -> Vec<u8> {
    (0..size_kb * 256)
        .map(|i| {
            let x = i.wrapping_mul(2654435761);
            ((x >> 16) & 0xFF) as u8
        })
        .collect()
}

fn mixed_data(size_kb: usize) -> Vec<u8> {
    let mut data = Vec::with_capacity(size_kb * 1024);
    let mut i = 0u32;
    while data.len() < size_kb * 1024 {
        if i % 3 == 0 {
            data.extend_from_slice(format!("key{i}=value_{i}\n").as_bytes());
        } else if i % 3 == 1 {
            data.extend_from_slice(format!("{i:08x}\n").as_bytes());
        } else {
            data.push((i & 0xFF) as u8);
        }
        i += 1;
    }
    data.truncate(size_kb * 1024);
    data
}

fn benchmark(name: &str, data: &[u8]) {
    let codec = BrotliCodec::new();
    println!(
        "\n=== {} ({} bytes, {:.1} KB) ===",
        name,
        data.len(),
        data.len() as f64 / 1024.0
    );

    for &q in &[2, 5, 8, 11] {
        let level = CompressionLevel::new(q as u8);

        // from_spec encoder
        let t0 = Instant::now();
        let spec_out = codec
            .compress(data, level)
            .unwrap_or_else(|e| panic!("spec q{q}: {e:?}"));
        let spec_time = t0.elapsed();
        let spec_ratio = spec_out.len() as f64 / data.len() as f64 * 100.0;

        // Verify round-trip
        let decompressed = codec
            .decompress(&spec_out, data.len() as u32)
            .unwrap_or_else(|e| panic!("spec decode q{q}: {e:?}"));
        let spec_ok = decompressed == data;

        println!(
            "  Q{:2} spec:   {:7} bytes ({:5.1}%) in {:5.2}s {}",
            q,
            spec_out.len(),
            spec_ratio,
            spec_time.as_secs_f64(),
            if spec_ok { "OK" } else { "FAIL" }
        );
    }

    // Vendored C reference (quality 11)
    let t0 = Instant::now();
    let vendored_out = codec
        .compress_with_options(data, BrotliOptions::default())
        .unwrap_or_else(|e| panic!("vendored: {e:?}"));
    let vendored_time = t0.elapsed();
    let vendored_ratio = vendored_out.len() as f64 / data.len() as f64 * 100.0;

    let vendored_ok = match codec.decompress(&vendored_out, data.len() as u32) {
        Ok(d) => d == data,
        Err(e) => {
            println!("         vendored decode error: {e}");
            false
        }
    };

    println!(
        "  Q11 vend:   {:7} bytes ({:5.1}%) in {:5.2}s {}",
        vendored_out.len(),
        vendored_ratio,
        vendored_time.as_secs_f64(),
        if vendored_ok { "OK" } else { "DECODE-FAIL" }
    );
}

fn main() {
    println!("Brotli encoder benchmark: from_spec vs vendored C reference");
    println!("============================================================");

    benchmark("CSV (100 KB)", &csv_data(100));
    benchmark("English text (100 KB)", &english_text(100));
    benchmark("Binary pseudo-random (100 KB)", &binary_data(100));
    benchmark("Mixed text/binary (100 KB)", &mixed_data(100));

    benchmark("CSV (500 KB)", &csv_data(500));
    benchmark("English text (500 KB)", &english_text(500));
}
