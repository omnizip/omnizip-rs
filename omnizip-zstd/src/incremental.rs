//! Incremental frame decoding — the task-57 final sliver.
//!
//! [`frame_span`] computes how many input bytes one zstd frame
//! occupies WITHOUT decoding it (frame header fields + a walk of
//! the 3-byte block headers), so a streaming decoder can hold
//! back truncated frames and emit the plaintext of every complete
//! frame the moment its last byte arrives.

use crate::decoder::ZstdDecoder;
use crate::ZstdError;

/// `Ok(None)` = a valid-so-far but truncated frame (feed more);
/// `Ok(Some(len))` = a complete frame occupies `input[..len]`;
/// `Err` = structurally invalid (more input cannot fix it).
///
/// Skippable frames count as frames with their own span.
///
/// # Errors
///
/// [`ZstdError::Corrupt`] on structurally invalid input.
pub fn frame_span(input: &[u8]) -> Result<Option<usize>, ZstdError> {
    let need = |have: usize, want: usize| -> Result<bool, ZstdError> {
        if have >= want {
            Ok(true)
        } else if want <= 4 * 1024 * 1024 || have + 16 >= want {
            // Missing bytes cannot be judged yet — only clearly
            // absurd wants (huge, with almost nothing buffered) are
            // corruption.
            Ok(false)
        } else {
            Err(ZstdError::Corrupt {
                reason: format!("frame header wants {want} bytes, have {have}"),
            })
        }
    };

    if input.len() < 4 {
        return Ok(None);
    }
    let magic = u32::from_le_bytes([input[0], input[1], input[2], input[3]]);
    if (magic & crate::constants::SKIPPABLE_MAGIC_MASK) == crate::constants::SKIPPABLE_MAGIC_BASE {
        if input.len() < 8 {
            return Ok(None);
        }
        let size = u32::from_le_bytes([input[4], input[5], input[6], input[7]]) as usize;
        let end = 8usize.checked_add(size).ok_or_else(|| ZstdError::Corrupt {
            reason: format!("skippable frame size {size} overflows usize"),
        })?;
        return if input.len() >= end {
            Ok(Some(end))
        } else {
            Ok(None)
        };
    }
    if magic != crate::constants::MAGIC_NUMBER {
        return Err(ZstdError::Corrupt {
            reason: format!("invalid ZSTD magic: 0x{magic:08X}"),
        });
    }

    // ---- Frame header (RFC 8878 §3.1.1.1) ----
    if !need(input.len(), 5)? {
        return Ok(None);
    }
    let fhd = input[4];
    let fcs_flag = usize::from(fhd >> 6);
    let single_segment = fhd & 0x20 != 0;
    let has_checksum = fhd & 0x04 != 0;
    let did_flag = usize::from(fhd & 0x03);

    let mut pos = 5;
    if !single_segment {
        if !need(input.len(), pos + 1)? {
            return Ok(None);
        }
        pos += 1; // window descriptor
    }
    let did_bytes = [0, 1, 2, 4][did_flag];
    if !need(input.len(), pos + did_bytes)? {
        return Ok(None);
    }
    pos += did_bytes;
    let fcs_bytes = match fcs_flag {
        0 => usize::from(single_segment),
        1 => 2,
        2 => 4,
        _ => 8,
    };
    if !need(input.len(), pos + fcs_bytes)? {
        return Ok(None);
    }
    pos += fcs_bytes;

    // ---- Block headers (§3.1.1.3): 3-byte LE, walk to last_block ----
    loop {
        if !need(input.len(), pos + 3)? {
            return Ok(None);
        }
        let h = u32::from_le_bytes([input[pos], input[pos + 1], input[pos + 2], 0]);
        let last_block = h & 1 != 0;
        let block_type = (h >> 1) & 0x03;
        let block_size = usize::try_from(h >> 3).unwrap_or(usize::MAX >> 3);
        if block_type == 3 {
            return Err(ZstdError::Corrupt {
                reason: "reserved block type 3".into(),
            });
        }
        pos += 3;
        // RLE blocks carry ONE byte regardless of the size field
        // (RFC 8878 §3.1.1.3.2.2); Raw and Compressed carry `size`.
        let payload = if block_type == 1 { 1 } else { block_size };
        if !need(input.len(), pos + payload)? {
            return Ok(None);
        }
        pos += payload;
        if last_block {
            break;
        }
    }
    if has_checksum {
        if !need(input.len(), pos + 4)? {
            return Ok(None);
        }
        pos += 4;
    }
    Ok(Some(pos))
}

/// Incremental zstd decoder over [`frame_span`]: `write` returns the
/// plaintext of every frame completed by that call (empty when the
/// buffered frame is still truncated). Output across all calls +
/// `finish` equals the one-shot [`crate::decompress`] exactly.
pub struct IncrementalDecoder {
    buf: Vec<u8>,
    finished: bool,
}

impl Default for IncrementalDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl IncrementalDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            finished: false,
        }
    }

    /// Feed compressed bytes; returns plaintext of any frames that
    /// became complete.
    ///
    /// # Errors
    ///
    /// [`ZstdError::Corrupt`] on structurally invalid input, or a
    /// decode failure inside a complete frame.
    pub fn write(&mut self, data: &[u8]) -> Result<Vec<u8>, ZstdError> {
        if self.finished {
            return Err(ZstdError::Corrupt {
                reason: "write after finish".into(),
            });
        }
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        while let Some(span) = frame_span(&self.buf)? {
            let (frame, tail) = self.buf.split_at(span);
            let mut decoder = ZstdDecoder::new();
            out.extend_from_slice(&decoder.decode_stream(frame)?);
            self.buf = tail.to_vec();
            if self.buf.is_empty() {
                break;
            }
        }
        Ok(out)
    }

    /// Finish: any buffered bytes are an incomplete frame (the
    /// format has no trailing trailer) — error unless empty.
    ///
    /// # Errors
    ///
    /// [`ZstdError::Corrupt`] when bytes remain buffered.
    pub fn finish(&mut self) -> Result<Vec<u8>, ZstdError> {
        self.finished = true;
        if self.buf.is_empty() {
            Ok(Vec::new())
        } else {
            Err(ZstdError::Corrupt {
                reason: format!("stream ends mid-frame: {} bytes buffered", self.buf.len()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::IncrementalDecoder;

    fn frames(parts: &[usize]) -> Vec<Vec<u8>> {
        // Independent frames of distinct repetitive content, via the
        // real encoder (covers compressed + checksummed frames).
        parts
            .iter()
            .enumerate()
            .map(|(i, &len)| {
                let body: Vec<u8> = std::iter::repeat(b'a' + i as u8).take(len).collect();
                crate::encoder::block::encode_frame_compressed(&body, 3).unwrap()
            })
            .collect()
    }

    #[test]
    fn incremental_emits_per_frame_and_matches_one_shot() {
        let parts = frames(&[1000, 5000, 700]);
        let mut stream = Vec::new();
        for f in &parts {
            stream.extend_from_slice(f);
        }
        let expected: Vec<u8> = parts
            .iter()
            .map(|f| crate::decompress(f, u32::MAX).unwrap())
            .collect::<Vec<_>>()
            .concat();

        // Byte-at-a-time feeding: after frame 1's last byte lands,
        // write() must emit its plaintext immediately.
        let mut d = IncrementalDecoder::new();
        let mut got = Vec::new();
        let mut emitted_mid_stream = 0;
        for &b in &stream {
            got.extend_from_slice(&d.write(std::slice::from_ref(&b)).unwrap());
            if !got.is_empty() {
                emitted_mid_stream += 1;
            }
        }
        got.extend_from_slice(&d.finish().unwrap());
        assert_eq!(got, expected);
        assert!(emitted_mid_stream >= 2, "no incremental emission happened");
    }

    #[test]
    fn truncation_is_detected_not_misdecoded() {
        let mut stream = frames(&[5000]).remove(0);
        let cut = stream.len() - 3;
        let mut d = IncrementalDecoder::new();
        let out = d.write(&stream[..cut]).unwrap();
        assert!(out.is_empty(), "truncated frame emitted output");
        stream.truncate(cut);
        assert!(d.finish().is_err(), "finish accepted a mid-frame cut");
    }

    #[test]
    fn adversarial_partitions_match() {
        let parts = frames(&[2000, 3000, 111]);
        let mut stream = Vec::new();
        for f in &parts {
            stream.extend_from_slice(f);
        }
        let baseline = crate::decompress(&stream, u32::MAX).unwrap();

        let mut d = IncrementalDecoder::new();
        let mut got = Vec::new();
        let mut i = 0;
        while i < stream.len() {
            let n = 29.min(stream.len() - i);
            got.extend_from_slice(&d.write(&stream[i..i + n]).unwrap());
            i += n;
        }
        got.extend_from_slice(&d.finish().unwrap());
        assert_eq!(got, baseline);
    }
}
