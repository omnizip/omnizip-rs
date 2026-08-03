//! FLAC CLI parity: our encoder → libFLAC CLI decoder → byte-compare.
//!
//! This is the canonical interop test: if libFLAC accepts our output
//! and produces byte-identical audio, we are spec-compliant.
//! Regression of this test means a codec-level break that needs to
//! be fixed before merge.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::io::Write;
use std::process::Command;

use omnizip_differential::wav::mono as mono_wav;

fn assert_parity(label: &str, wav: &[u8]) {
    if omnizip_differential::which("flac").ok().flatten().is_none() {
        eprintln!("[skip] {label}: flac CLI not installed");
        return;
    }
    let samples = &wav[44..];
    let sample_count = samples.len() / 2;

    // Extract the sample rate from the WAV header so we use the same
    // rate in the encode parameters as in the WAV. This way the
    // libFLAC-decoded WAV matches the input byte-for-byte.
    let sr = u32::from_le_bytes(wav[24..28].try_into().unwrap());

    use omnizip_flac::encoder::encode_stream;
    use omnizip_flac::pcm_header::{Endianness, PcmParams};
    let params = PcmParams {
        sample_rate: sr,
        channels: 1,
        bits_per_sample: 16,
        endianness: Endianness::LittleEndian,
        sample_count: sample_count as u32,
    };
    let encoded = encode_stream(samples, &params).expect("encode");

    let in_path = std::env::temp_dir().join(format!(
        "omnizip_flac_parity_{}_{}.flac",
        std::process::id(),
        label.replace(|c: char| !c.is_alphanumeric(), "_")
    ));
    {
        let mut f = std::fs::File::create(&in_path).expect("create temp FLAC");
        f.write_all(&encoded).expect("write temp FLAC");
    }
    let out = Command::new("flac")
        .arg("-d")
        .arg("-f")
        .arg("-o")
        .arg("-")
        .arg(&in_path)
        .output()
        .expect("spawn flac");
    let _ = std::fs::remove_file(&in_path);
    if !out.status.success() {
        eprintln!("libFLAC stderr: {}", String::from_utf8_lossy(&out.stderr));
        panic!("libFLAC failed to decode our {label} output");
    }

    if out.stdout != wav {
        panic!(
            "{label}: libFLAC-decoded WAV does not byte-match the original\n\
             input size: {}, decoded size: {}\n\
             stderr: {}",
            wav.len(),
            out.stdout.len(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    eprintln!("[ok] {label}: libFLAC decodes our output byte-identically ({} bytes)", out.stdout.len());
}

// `which` is provided by `omnizip_differential::which` — no local copy needed.

#[test]
fn parity_constant_zero() {
    // All-zero signal — CONSTANT subframe.
    let wav = mono_wav(192, 8000, |_| 0);
    assert_parity("constant-zero", &wav);
}

#[test]
fn parity_verbatim_noise() {
    // Random 16-bit samples — VERBATIM subframe.
    let wav = mono_wav(192, 8000, |i| {
        i16::from_le_bytes([(i as u8).wrapping_mul(31), (i as u8).wrapping_mul(17)])
    });
    assert_parity("verbatim-noise", &wav);
}

#[test]
fn parity_fixed_linear_ramp() {
    // Linear ramp — FIXED order 1 or 2.
    let wav = mono_wav(192, 8000, |i| (i * 2) as i16);
    assert_parity("fixed-ramp", &wav);
}

#[test]
fn parity_sine_short() {
    // 192-sample sine — too short for LPC.
    let wav = mono_wav(192, 8000, |i| {
        ((i as f64 * 440.0 * std::f64::consts::TAU / 8000.0).sin() * 30000.0) as i16
    });
    assert_parity("sine-short", &wav);
}

#[test]
fn parity_sine_long() {
    // 4096-sample sine — exercises multi-block-size selection.
    let wav = mono_wav(4096, 44_100, |i| {
        ((i as f64 * 440.0 * std::f64::consts::TAU / 44_100.0).sin() * 30000.0) as i16
    });
    assert_parity("sine-long", &wav);
}

#[test]
fn parity_sine_3_seconds() {
    // 131 072-sample sine — exercises multi-frame encoding.
    let wav = mono_wav(131_072, 44_100, |i| {
        ((i as f64 * 440.0 * std::f64::consts::TAU / 44_100.0).sin() * 30000.0) as i16
    });
    assert_parity("sine-3s", &wav);
}
