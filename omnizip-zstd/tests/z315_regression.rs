//! Regression gates for issue #315's residual: the decoder once
//! mis-decoded VALID frames its own encoder produced on mixed
//! text+binary+nul-pad content (Fastest/Fast/Default/Better; 163-byte
//! delta-debugged repro). The decoder fixes since cleared it — these
//! tests keep it cleared: the pinned blob, the content SHAPE class it
//! was minimized from, and a system-CLI differential must all hold.

use omnizip_codecs::CompressionLevel;
use omnizip_zstd::ZstdLevel;

/// The 163-byte repro from BUGREPORT-zstd-315-residual.md (base64 in
/// the report; bytes inlined here). Text with repeats + binary tail +
/// nul padding.
const Z315_BLOB: [u8; 163] = [
    0x2f, 0x08, 0xce, 0x18, 0x01, 0x00, 0x00, 0x00, 0x04, 0xa5, 0x00, 0x00,
    0x00, 0x64, 0x75, 0x70, 0x6c, 0x69, 0x63, 0x61, 0x74, 0x65, 0x20, 0x69,
    0x6e, 0x6c, 0x69, 0x6e, 0x65, 0x20, 0x63, 0x6f, 0x6e, 0x74, 0x65, 0x6e,
    0x68, 0x65, 0x20, 0x73, 0x61, 0x6d, 0x65, 0x20, 0x32, 0x30, 0x30, 0x2d,
    0x69, 0x73, 0x68, 0x20, 0x62, 0x79, 0x74, 0x65, 0x73, 0x20, 0x69, 0x6e,
    0x20, 0x74, 0x68, 0x72, 0x65, 0x65, 0x20, 0x66, 0x69, 0x6c, 0x65, 0x73,
    0x2c, 0x20, 0x73, 0x6f, 0x20, 0x74, 0x68, 0x65, 0x20, 0x77, 0x72, 0x69,
    0x74, 0x65, 0x72, 0x27, 0x65, 0x73, 0x20, 0x6f, 0x6e, 0x20, 0x65, 0x76,
    0x65, 0x72, 0x79, 0x20, 0x72, 0x65, 0x61, 0x6c, 0x69, 0x73, 0x74, 0x69,
    0x63, 0x20, 0x74, 0x72, 0x65, 0x65, 0x2e, 0x20, 0x50, 0x61, 0x64, 0x00,
    0x00, 0x00, 0x05, 0xe9, 0x40, 0x81, 0x2f, 0x08, 0xce, 0x01, 0x00, 0x00,
    0x00, 0x00, 0xd2, 0x7f, 0xef, 0x4f, 0xcc, 0x0d, 0x85, 0xbf, 0xc4, 0x6a,
    0x72, 0x68, 0x8d, 0xe4, 0x6b, 0x71, 0xfd, 0xf9, 0xf6, 0xa6, 0xee, 0x4b,
    0x08, 0x23, 0x62, 0x29, 0xc9, 0xde, 0x01,
];

fn all_levels() -> [(u8, ZstdLevel); 5] {
    [
        (1, ZstdLevel::Fastest),
        (3, ZstdLevel::Fast),
        (6, ZstdLevel::Default),
        (12, ZstdLevel::Better),
        (22, ZstdLevel::Best),
    ]
}

fn corpora() -> Vec<(&'static str, Vec<u8>)> {
    let mut v: Vec<(&'static str, Vec<u8>)> = Vec::new();
    v.push(("z315-blob", Z315_BLOB.to_vec()));
    v.push(("z315-head", Z315_BLOB[..120].to_vec()));
    v.push(("z315-tail", Z315_BLOB[120..].to_vec()));
    // The shape class the blob was minimized from: heavy text repeats,
    // nul padding, high-entropy tail.
    let mut shape: Vec<u8> =
        b"duplicate inline contenh same 200-ish bytes in three files, so the writer'es on every realistic tree."
            .to_vec();
    shape.extend_from_slice(&[0u8; 43]);
    shape.extend_from_slice(&Z315_BLOB[120..]);
    v.push(("z315-shape", shape));
    let mut csvish: Vec<u8> = Vec::new();
    for i in 0..2000 {
        csvish.extend_from_slice(format!("{i},name{i},city{}\n", i % 37).as_bytes());
    }
    v.push(("csv", csvish));
    v.push(("rle", vec![0x5a; 65_536]));
    v.push(("empty", Vec::new()));
    v
}

/// The exact #315 repro: every level must round-trip the pinned blob.
#[test]
fn z315_blob_round_trips_all_levels() {
    for (num, lvl) in all_levels() {
        let frame = omnizip_zstd::compress(&Z315_BLOB, lvl)
            .unwrap_or_else(|e| panic!("L{num} encode: {e:?}"));
        let out = omnizip_zstd::decompress(&frame, u32::MAX)
            .unwrap_or_else(|e| panic!("L{num} decode: {e:?}"));
        assert_eq!(out, Z315_BLOB, "L{num} round-trip diverged");
    }
}

/// The decoder must agree with the encoder on the whole shape class —
/// the old failure was decoder-vs-encoder divergence on VALID frames.
#[test]
fn mixed_shapes_self_round_trip_all_levels() {
    for (name, data) in corpora() {
        for (num, lvl) in all_levels() {
            let frame = omnizip_zstd::compress(&data, lvl)
                .unwrap_or_else(|e| panic!("{name} L{num} encode: {e:?}"));
            let out = omnizip_zstd::decompress(&frame, u32::MAX)
                .unwrap_or_else(|e| panic!("{name} L{num} decode: {e:?}"));
            assert_eq!(out, data, "{name} L{num} round-trip diverged");
        }
    }
}

fn system_zstd_available() -> bool {
    std::process::Command::new("zstd")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn pipe(binary: &str, args: &[&str], input: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write as _;
    let mut child = std::process::Command::new(binary)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    child.stdin.take().unwrap().write_all(input).ok()?;
    let out = child.wait_with_output().ok()?;
    out.status.success().then(|| out.stdout)
}

/// Full differential against the reference CLI, both directions:
/// reference frames must decode through us, our frames through it.
#[test]
fn differential_with_system_zstd_on_issue315_shapes() {
    if !system_zstd_available() {
        eprintln!("skipping: system zstd not found");
        return;
    }
    for (name, data) in corpora() {
        for lvl in ["-1", "-3", "-6", "-12", "-19"] {
            let Some(frame) = pipe("zstd", &["-q", "-c", lvl], &data) else {
                continue;
            };
            let got = omnizip_zstd::decompress(&frame, u32::MAX)
                .unwrap_or_else(|e| panic!("{name} ref {lvl} frame decode: {e:?}"));
            assert_eq!(got, data, "{name} ref {lvl} frame diverged");
        }
        for (num, lvl) in all_levels() {
            let frame = omnizip_zstd::compress(&data, lvl)
                .unwrap_or_else(|e| panic!("{name} L{num} encode: {e:?}"));
            let out = pipe("zstd", &["-q", "-c", "-d"], &frame)
                .unwrap_or_else(|| panic!("{name} L{num}: reference rejected our frame"));
            assert_eq!(out, data, "{name} L{num} reference decode diverged");
        }
    }
}

/// The codec-tier entry point must round-trip the blob too — the tier
/// is what LimniFS calls.
#[test]
fn z315_blob_codec_tier_round_trip() {
    use omnizip_codecs::Codec;
    let codec = omnizip_zstd::ZstdCodec;
    for lv in [1u8, 3, 6, 12, 22] {
        let level = CompressionLevel::new(lv);
        let frame = codec.compress(&Z315_BLOB, level).unwrap();
        let out = codec.decompress(&frame, Z315_BLOB.len() as u32).unwrap();
        assert_eq!(out, Z315_BLOB, "codec tier lv{lv} diverged");
    }
}
