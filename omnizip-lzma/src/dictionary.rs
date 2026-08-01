//! Circular-buffer dictionary — the LZMA sliding window.
//!
//! Ported from `omnizip/lib/omnizip/algorithms/lzma/dictionary.rb` (70 LOC,
//! MIT, Ribose Inc.). The decoder writes decompressed bytes here and reads
//! from it when expanding matches (`LZ77` back-references).
//!
//! ## Design choice vs the Ruby
//!
//! The Ruby uses a `String` and trims when it grows past `size`. That's
//! `O(n)` per trim and `O(n²)` for streams near the boundary. The Rust
//! port uses a fixed-size circular buffer with `O(1)` append and read.
//! The trade-off is more careful index arithmetic at access time.

#![forbid(unsafe_code)]

use crate::LzmaError;

/// Sliding-window dictionary used by the LZMA decoder.
///
/// Capacity is fixed at construction; bytes past the window are
/// overwritten, preserving only the last `size` bytes for back-reference
/// resolution.
#[derive(Debug)]
pub struct Dictionary {
    buffer: Vec<u8>,
    size: usize,
    /// Total bytes ever appended. Monotonic; used by callers that need
    /// the absolute stream position.
    position: u64,
    /// Index in `buffer` of the next write. Equal to `position % size`
    /// when `position >= size`; tracks the ring head.
    head: usize,
    /// Number of bytes currently held. Equal to `min(position, size)`.
    fullness: usize,
}

impl Dictionary {
    /// Construct a dictionary with capacity `size` bytes. Allocates
    /// upfront; subsequent appends are `O(1)` amortised.
    #[must_use]
    pub fn new(size: usize) -> Self {
        Self {
            buffer: vec![0u8; size],
            size,
            position: 0,
            head: 0,
            fullness: 0,
        }
    }

    /// The configured capacity.
    #[must_use]
    pub const fn size(&self) -> usize {
        self.size
    }

    /// Absolute stream position (number of bytes ever appended).
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }

    /// Number of valid bytes currently in the buffer.
    #[must_use]
    pub const fn fullness(&self) -> usize {
        self.fullness
    }

    /// Append a single byte, evicting the oldest if the window is full.
    pub fn append_byte(&mut self, byte: u8) {
        if self.size == 0 {
            // Degenerate: caller asked for a zero-byte dictionary.
            // Position still advances; nothing is stored.
            self.position += 1;
            return;
        }
        self.buffer[self.head] = byte;
        self.head = (self.head + 1) % self.size;
        if self.fullness < self.size {
            self.fullness += 1;
        }
        self.position += 1;
    }

    /// Append a slice of bytes.
    pub fn append(&mut self, data: &[u8]) {
        for &b in data {
            self.append_byte(b);
        }
    }

    /// Read `length` bytes starting `distance` bytes back from the
    /// current head. Used to expand LZ77 matches; the bytes may overlap
    /// the head when `length > distance`.
    ///
    /// # Errors
    ///
    /// Returns [`LzmaError::Corrupt`] if `distance` exceeds the bytes
    /// currently in the dictionary (the stream is referencing memory
    /// that was never written).
    pub fn copy_match(&mut self, distance: u32, length: u32) -> Result<(), LzmaError> {
        let dist = usize::try_from(distance).map_err(|_| LzmaError::Corrupt {
            reason: format!("match distance {distance} exceeds usize"),
        })?;
        let len = usize::try_from(length).map_err(|_| LzmaError::Corrupt {
            reason: format!("match length {length} exceeds usize"),
        })?;
        if dist == 0 {
            return Err(LzmaError::Corrupt {
                reason: "match distance 0 is invalid".into(),
            });
        }
        if dist > self.fullness {
            return Err(LzmaError::Corrupt {
                reason: format!("match distance {dist} exceeds dictionary fullness {}", self.fullness),
            });
        }
        if self.size == 0 {
            return Err(LzmaError::Corrupt {
                reason: "copy_match on zero-capacity dictionary".into(),
            });
        }

        // Compute the index where reading starts. The last byte appended
        // is at (head + size - 1) % size; distance=1 means "the byte we
        // just wrote", so we read from (head - distance + size) % size.
        for _ in 0..len {
            let src = (self.head + self.size - dist) % self.size;
            let byte = self.buffer[src];
            self.append_byte(byte);
        }
        Ok(())
    }

    /// Get the byte at `distance` back from the current head, without
    /// modifying state. Returns `None` if `distance > fullness`.
    #[must_use]
    pub fn byte_at_distance(&self, distance: u32) -> Option<u8> {
        let dist = usize::try_from(distance).ok()?;
        if dist == 0 || dist > self.fullness || self.size == 0 {
            return None;
        }
        let idx = (self.head + self.size - dist) % self.size;
        Some(self.buffer[idx])
    }

    /// Snapshot the dictionary contents as a `Vec<u8>`. Bytes are in
    /// insertion order (oldest to newest), excluding any evicted bytes.
    #[must_use]
    pub fn snapshot(&self) -> Vec<u8> {
        if self.fullness < self.size {
            // Buffer hasn't wrapped: valid bytes are 0..fullness.
            self.buffer[..self.fullness].to_vec()
        } else {
            // Buffer has wrapped: valid bytes are head..size followed by 0..head.
            let mut out = Vec::with_capacity(self.size);
            out.extend_from_slice(&self.buffer[self.head..]);
            out.extend_from_slice(&self.buffer[..self.head]);
            out
        }
    }

    /// Clear all state, keeping the allocated capacity.
    pub fn reset(&mut self) {
        self.position = 0;
        self.head = 0;
        self.fullness = 0;
        // Don't zero the buffer; consumers never read past `fullness`.
    }
}

impl Default for Dictionary {
    fn default() -> Self {
        Self::new(4096)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_records_bytes_in_order() {
        let mut d = Dictionary::new(16);
        d.append(b"hello");
        assert_eq!(d.snapshot(), b"hello");
        assert_eq!(d.position(), 5);
        assert_eq!(d.fullness(), 5);
    }

    #[test]
    fn append_overwrites_oldest_when_full() {
        let mut d = Dictionary::new(4);
        d.append(b"ABCDE");
        // 'A' is evicted; the window holds BCDE.
        assert_eq!(d.snapshot(), b"BCDE");
        assert_eq!(d.position(), 5);
        assert_eq!(d.fullness(), 4);
    }

    #[test]
    fn copy_match_expands_non_overlapping() {
        let mut d = Dictionary::new(16);
        d.append(b"abcde");
        // distance=5, length=3 → copy "abc"
        d.copy_match(5, 3).expect("copy");
        assert_eq!(d.snapshot(), b"abcdeabc");
    }

    #[test]
    fn copy_match_handles_overlap_rle() {
        // Classic RLE pattern: distance=1, length=4 repeats one byte.
        let mut d = Dictionary::new(16);
        d.append(b"X");
        d.copy_match(1, 4).expect("copy");
        assert_eq!(d.snapshot(), b"XXXXX");
    }

    #[test]
    fn copy_match_rejects_excessive_distance() {
        let mut d = Dictionary::new(16);
        d.append(b"hi");
        let err = d.copy_match(10, 2).unwrap_err();
        assert!(matches!(err, LzmaError::Corrupt { .. }));
    }

    #[test]
    fn copy_match_rejects_zero_distance() {
        let mut d = Dictionary::new(16);
        d.append(b"hi");
        let err = d.copy_match(0, 2).unwrap_err();
        assert!(matches!(err, LzmaError::Corrupt { .. }));
    }

    #[test]
    fn byte_at_distance_returns_last_byte_at_dist_one() {
        let mut d = Dictionary::new(8);
        d.append(b"abc");
        assert_eq!(d.byte_at_distance(1), Some(b'c'));
        assert_eq!(d.byte_at_distance(2), Some(b'b'));
        assert_eq!(d.byte_at_distance(3), Some(b'a'));
        assert_eq!(d.byte_at_distance(4), None);
    }

    #[test]
    fn reset_clears_state_preserves_capacity() {
        let mut d = Dictionary::new(8);
        d.append(b"hello");
        d.reset();
        assert_eq!(d.position(), 0);
        assert_eq!(d.fullness(), 0);
        assert_eq!(d.size(), 8);
        assert_eq!(d.snapshot(), b"");
    }

    #[test]
    fn snapshot_correct_after_multiple_wraps() {
        let mut d = Dictionary::new(3);
        d.append(b"ABCDEF"); // window slides over 6 bytes, capacity 3
        assert_eq!(d.snapshot(), b"DEF");
        assert_eq!(d.position(), 6);
    }
}
