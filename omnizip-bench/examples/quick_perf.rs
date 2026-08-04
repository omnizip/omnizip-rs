//! Quick perf smoke for the LZMA / FLAC / ZPAQ optimisations.
//!
//! Runs each encoder twice on a synthetic but representative input,
//! measuring wall-clock throughput. Prints MB/s for each codec.
//!
//! This is a *smoke* bench — not a rigorous Silesia-grade sweep.
//! Use `cargo run --release` (NOT `--bench`) to skip the formal
//! criterion harness and get a one-shot snapshot.

use std::hint::black_box;
use std::time::Instant;

use omnizip_codecs::{Codec, CompressionLevel};
use omnizip_deflate::DeflateCodec;
use omnizip_lzma::LzmaCodec;
use omnizip_zpaq::ZpaqCodec;

fn main() {
    // 64 KiB — small enough to keep a single LZMA2 chunk's compressed
    // payload under the format's u16 cap. Larger inputs work too via
    // chunking, but this smoke bench keeps things simple.
    let text = make_text(64 * 1024);
    println!("input: {} bytes (synthetic enwik-like text)\n", text.len());

    bench_lzma(&text);
    bench_zpaq(&text);
    bench_deflate(&text);
}

fn bench_lzma(text: &[u8]) {
    let codec = LzmaCodec::new();
    for &level in &[1u8, 3, 6, 9] {
        let t = Instant::now();
        let out = codec
            .compress(black_box(text), CompressionLevel::new(level))
            .expect("compress");
        let elapsed = t.elapsed();
        let mbps = (text.len() as f64 / 1e6) / elapsed.as_secs_f64();
        let ratio = (text.len() as f64) / (out.len() as f64);
        println!(
            "lzma-{level}: {mbps:>6.1} MB/s  ratio={ratio:>5.2}×  ({} → {} bytes) in {:.2}s",
            text.len(),
            out.len(),
            elapsed.as_secs_f64()
        );
    }
}

fn bench_zpaq(text: &[u8]) {
    let codec = ZpaqCodec;
    for &level in &[1u8, 3, 5] {
        let t = Instant::now();
        let out = codec
            .compress(black_box(text), CompressionLevel::new(level))
            .expect("compress");
        let elapsed = t.elapsed();
        let mbps = (text.len() as f64 / 1e6) / elapsed.as_secs_f64();
        let ratio = (text.len() as f64) / (out.len() as f64);
        println!(
            "zpaq-{level}: {mbps:>6.1} MB/s  ratio={ratio:>5.2}×  ({} → {} bytes) in {:.2}s",
            text.len(),
            out.len(),
            elapsed.as_secs_f64()
        );
    }
}

fn bench_deflate(text: &[u8]) {
    let codec = DeflateCodec::new();
    for &level in &[1u8, 6, 9] {
        let t = Instant::now();
        let out = codec
            .compress(black_box(text), CompressionLevel::new(level))
            .expect("compress");
        let elapsed = t.elapsed();
        let mbps = (text.len() as f64 / 1e6) / elapsed.as_secs_f64();
        let ratio = (text.len() as f64) / (out.len() as f64);
        println!(
            "deflate-{level}: {mbps:>5.1} MB/s  ratio={ratio:>5.2}×  ({} → {} bytes) in {:.2}s",
            text.len(),
            out.len(),
            elapsed.as_secs_f64()
        );
    }
}

/// Deterministic enwik-like synthetic text: real English word
/// distribution, repeated paragraphs, headings, and tags.
fn make_text(target_bytes: usize) -> Vec<u8> {
    let words = [
        "the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog",
        "hello", "world", "compression", "rust", "performance", "codec",
        "flac", "zpaq", "lzma", "deflate", "brotli", "zstd",
        "algorithm", "byte", "encoding", "decoding", "stream",
        "header", "footer", "block", "frame", "checksum",
    ];
    let mut out = Vec::with_capacity(target_bytes);
    let mut seed: u64 = 0xDEAD_BEEF_CAFE_BABE;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    while out.len() < target_bytes {
        let paragraph_len = 30 + (next() % 80) as usize;
        for _ in 0..paragraph_len {
            let w = words[(next() as usize) % words.len()];
            out.extend_from_slice(w.as_bytes());
            out.push(b' ');
        }
        out.push(b'\n');
        if (next() & 0x7) == 0 {
            out.extend_from_slice(b"== Heading ==\n");
        }
    }
    out.truncate(target_bytes);
    out
}
