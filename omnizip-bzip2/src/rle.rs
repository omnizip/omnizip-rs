//! RLE1 — initial run-length encoding stage of `BZip2`.
//!
//! Port of `omnizip/lib/omnizip/algorithms/bzip2/rle.rb`.
//!
//! BZip2-specific RLE that collapses runs of 4 or more identical bytes into
//! 4 copies followed by a count byte (0–255), encoding total run lengths of
//! 4–259. Runs shorter than 4 are left untouched, which avoids ambiguity at
//! decode time.

/// Maximum encodable run length (4 + 255).
pub const MAX_RUN_LENGTH: usize = 259;

/// Minimum run length that triggers encoding.
pub const MIN_RUN_LENGTH: usize = 4;

/// Encode `data` using `BZip2` RLE1.
///
/// Returns the RLE1 stream. Runs of 4–259 identical bytes collapse to
/// `[byte; 4]` plus one count byte holding `run - 4`.
#[must_use]
pub fn rle_encode(data: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    if data.is_empty() {
        return result;
    }

    let mut i = 0;
    while i < data.len() {
        let byte = data[i];
        let run = count_run(data, i);

        if run >= MIN_RUN_LENGTH {
            let extra = run - MIN_RUN_LENGTH;
            let clamped = extra.min(u8::MAX as usize);
            // Emit 4 copies + extra count
            result.extend_from_slice(&[byte, byte, byte, byte]);
            result.push(clamped as u8);
            i += MIN_RUN_LENGTH + clamped;
        } else {
            result.push(byte);
            i += 1;
        }
    }

    result
}

/// Decode `BZip2` RLE1 stream produced by [`rle_encode`].
///
/// # Errors
///
/// Returns an error message string if the stream is truncated (a run marker
/// promises a count byte that isn't there).
pub fn rle_decode(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    if data.is_empty() {
        return Ok(result);
    }

    let mut i = 0;
    // Tracks how many more bytes we must skip before considering another run.
    // After emitting the extra copies of a run, the next 3 occurrences of the
    // run byte are part of the expanded run, not a new run trigger.
    let mut skip_count: usize = 0;

    while i < data.len() {
        let byte = data[i];
        result.push(byte);
        i += 1;

        if skip_count > 0 {
            skip_count -= 1;
            continue;
        }

        // A run is signalled by 4 consecutive identical bytes at the tail of
        // the output so far.
        if result.len() >= 4 && tail_all_equal(&result, byte, 4) {
            if i >= data.len() {
                return Err("RLE1 stream truncated: missing run count byte".to_string());
            }
            let count = data[i] as usize;
            i += 1;
            result.extend(std::iter::repeat(byte).take(count));
            // The Ruby reference sets skip_count = 3 so the 3 trailing copies
            // (positions 2/3/4 of the [byte,byte,byte,byte] marker) are not
            // mistaken for a new run.
            skip_count = 3;
        }
    }

    Ok(result)
}

/// Count the length of the run of identical bytes starting at `start`.
fn count_run(data: &[u8], start: usize) -> usize {
    let byte = data[start];
    let limit = (start + MAX_RUN_LENGTH).min(data.len());
    let mut count = 1;
    for &b in &data[start + 1..limit] {
        if b != byte {
            break;
        }
        count += 1;
    }
    count
}

/// True if the last `count` bytes of `buf` are all equal to `byte`.
fn tail_all_equal(buf: &[u8], byte: u8, count: usize) -> bool {
    if buf.len() < count {
        return false;
    }
    buf[buf.len() - count..].iter().all(|&b| b == byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_round_trips() {
        let enc = rle_encode(b"");
        assert!(enc.is_empty());
        let dec = rle_decode(&enc).unwrap();
        assert!(dec.is_empty());
    }

    #[test]
    fn short_runs_pass_through() {
        // Runs < 4 are untouched.
        let data = b"aaabbbc";
        let enc = rle_encode(data);
        assert_eq!(enc, data);
        let dec = rle_decode(&enc).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn long_run_encodes() {
        let data = vec![0x41u8; 10];
        let enc = rle_encode(&data);
        // 4 copies + count(=6)
        assert_eq!(enc, vec![0x41, 0x41, 0x41, 0x41, 6]);
        let dec = rle_decode(&enc).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn run_at_max_boundary() {
        let data = vec![0x00u8; 259];
        let enc = rle_encode(&data);
        assert_eq!(enc, vec![0x00, 0x00, 0x00, 0x00, 255]);
        let dec = rle_decode(&enc).unwrap();
        assert_eq!(dec.len(), 259);
        assert!(dec.iter().all(|&b| b == 0));
    }

    #[test]
    fn run_exceeding_max_splits() {
        // 260 identical bytes must split into a 259-run + a lone byte.
        let data = vec![0x55u8; 260];
        let enc = rle_encode(&data);
        // [0x55;4], 255, then one more 0x55 (single, since < 4).
        assert_eq!(enc, vec![0x55, 0x55, 0x55, 0x55, 255, 0x55]);
        let dec = rle_decode(&enc).unwrap();
        assert_eq!(dec, data);
    }

    #[test]
    fn round_trip_mixed() {
        let data: Vec<u8> = [
            vec![1u8, 2, 3],
            vec![0xAA; 100],
            vec![4, 5, 6, 7],
            vec![0xBB; 5],
        ]
        .concat();
        let enc = rle_encode(&data);
        let dec = rle_decode(&enc).unwrap();
        assert_eq!(dec, data);
    }
}
