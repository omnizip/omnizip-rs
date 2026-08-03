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

/// Generate a WAV byte vector for `n` mono 16-bit samples from `f(i)`.
/// The resulting WAV has the given `sample_rate` AND we use the same
/// `sample_rate` in the encode parameters, so the libFLAC-decoded
/// WAV has the same header as the input.
fn mono_wav<F: Fn(usize) -> i16>(n: usize, sr: u32, f: F) -> Vec<u8> {
    let data: Vec<u8> = (0..n).flat_map(|i| f(i).to_le_bytes()).collect();
    let mut hdr = Vec::with_capacity(44);
    hdr.extend_from_slice(b"RIFF");
    hdr.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    hdr.extend_from_slice(b"WAVEfmt ");
    hdr.extend_from_slice(&16u32.to_le_bytes());
    hdr.extend_from_slice(&1u16.to_le_bytes()); // PCM
    hdr.extend_from_slice(&1u16.to_le_bytes()); // mono
    hdr.extend_from_slice(&sr.to_le_bytes());
    hdr.extend_from_slice(&(sr * 2).to_le_bytes());
    hdr.extend_from_slice(&2u16.to_le_bytes());
    hdr.extend_from_slice(&16u16.to_le_bytes());
    hdr.extend_from_slice(b"data");
    hdr.extend_from_slice(&(data.len() as u32).to_le_bytes());
    hdr.extend_from_slice(&data);
    hdr
}

fn assert_parity(label: &str, wav: &[u8]) {
    if which("flac").is_none() {
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

fn which(cmd: &str) -> Option<String> {
    let out = Command::new("which").arg(cmd).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        None
    }
}

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
