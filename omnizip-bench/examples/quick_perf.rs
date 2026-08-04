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
use omnizip_flac::encoder::encode_stream as flac_encode_stream;
use omnizip_flac::pcm_header::{Endianness, PcmParams};
use omnizip_lz4::{Lz4FastCodec, Lz4HcCodec};
use omnizip_lzma::LzmaCodec;
use omnizip_zpaq::ZpaqCodec;
use omnizip_zstd::ZstdCodec;

fn main() {
    // 64 KiB — small enough to keep a single LZMA2 chunk's compressed
    // payload under the format's u16 cap. Larger inputs work too via
    // chunking, but this smoke bench keeps things simple.
    let text = make_text(64 * 1024);
    println!("input: {} bytes (synthetic enwik-like text)\n", text.len());

    bench_lzma(&text);
    bench_zstd(&text);
    bench_lz4(&text);
    bench_zpaq(&text);
    bench_deflate(&text);

    // FLAC takes raw PCM, not text. Synthesise a sine wave.
    let pcm = make_sine_pcm(4096 * 4, 8000, 440.0);
    println!("\ninput: {} bytes (4 KiB-block sine PCM)\n", pcm.len());
    bench_flac(&pcm);
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

fn bench_zstd(text: &[u8]) {
    let codec = ZstdCodec::new();
    // Note: ZSTD has a perf cliff on text inputs ≥ 8KB at all levels
    // (likely O(N²) in the block/sequence assembly). Filed as a TODO;
    // for now we only bench on small inputs to avoid stalling.
    let bench_input = &text[..text.len().min(4096)];
    for &level in &[1u8, 3, 9] {
        let t = Instant::now();
        let out = codec
            .compress(black_box(bench_input), CompressionLevel::new(level))
            .expect("compress");
        let elapsed = t.elapsed();
        let mbps = (bench_input.len() as f64 / 1e6) / elapsed.as_secs_f64();
        let ratio = (bench_input.len() as f64) / (out.len() as f64);
        println!(
            "zstd-{level} (4KB): {mbps:>5.1} MB/s  ratio={ratio:>5.2}×  in {:.3}s",
            elapsed.as_secs_f64()
        );
    }
}

fn bench_lz4(text: &[u8]) {
    let fast = Lz4FastCodec;
    let hc = Lz4HcCodec;
    let cases: &[(&str, &dyn Codec, u8)] = &[
        ("lz4-1", &fast, 1),
        ("lz4-9", &fast, 9),
        ("lz4hc-12", &hc, 12),
    ];
    for (label, codec, level) in cases {
        let t = Instant::now();
        let out = codec
            .compress(black_box(text), CompressionLevel::new(*level))
            .expect("compress");
        let elapsed = t.elapsed();
        let mbps = (text.len() as f64 / 1e6) / elapsed.as_secs_f64();
        let ratio = (text.len() as f64) / (out.len() as f64);
        println!(
            "{label}: {mbps:>5.1} MB/s  ratio={ratio:>5.2}×  ({} → {} bytes) in {:.2}s",
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

fn bench_flac(pcm: &[u8]) {
    let params = PcmParams {
        sample_rate: 8_000,
        channels: 1,
        bits_per_sample: 16,
        endianness: Endianness::LittleEndian,
        sample_count: (pcm.len() / 2) as u32,
    };
    let t = Instant::now();
    let out = flac_encode_stream(black_box(pcm), &params).expect("flac encode");
    let elapsed = t.elapsed();
    let mbps = (pcm.len() as f64 / 1e6) / elapsed.as_secs_f64();
    let ratio = (pcm.len() as f64) / (out.len() as f64);
    println!(
        "flac:       {mbps:>5.1} MB/s  ratio={ratio:>5.2}×  ({} → {} bytes) in {:.2}s",
        pcm.len(),
        out.len(),
        elapsed.as_secs_f64()
    );
}

fn make_sine_pcm(n: usize, sr: u32, freq: f64) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / sr as f64;
        let s = (t * freq * std::f64::consts::TAU).sin() * 30_000.0;
        let v = (s as i16).to_le_bytes();
        pcm.push(v[0]);
        pcm.push(v[1]);
    }
    pcm
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
