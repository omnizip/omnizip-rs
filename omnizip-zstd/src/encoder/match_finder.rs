//! ZSTD match finder — hash-table-based LZ77 matching.
//!
//! Ported from `~/src/external/zstd/lib/compress/zstd_fast.c`
//! (`ZSTD_compressBlock_fast_generic`). Produces a sequence store
//! (literal runs + match descriptors) that the block encoder feeds to
//! the literals and sequences entropy coders.
//!
//! ## Algorithm
//!
//! The fast parser uses a single hash table (no chains). At each
//! position, it hashes the next 4 bytes, looks up the hash to find a
//! candidate match, verifies the match by comparing 4 bytes, and
//! extends the match forward. Matches are also checked backward to
//! absorb preceding literal bytes.
//!
//! Repeat-offset matches (repcodes) are checked at every position:
//! if `ip[0..4] == (ip - rep)[0..4]`, emit a repcode sequence with
//! zero literal length.

#![forbid(unsafe_code)]

use crate::constants::BLOCK_MAX_SIZE;

/// Multiplicative hash prime for 4-byte hash (matches C
/// `prime4bytes`).
const PRIME4_BYTES: u32 = 2_654_435_761;

/// Minimum match length for ZSTD fast mode.
const MIN_MATCH: usize = 4;

/// Number of repeat offsets ZSTD tracks.
pub const REP_NUM: usize = 3;

/// Hash 4 bytes at `data[pos..]` into `hBits` bits.
/// Matches C's `ZSTD_hash4Ptr`.
fn hash4(data: &[u8], pos: usize, h_bits: u32) -> u32 {
    let val = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
    val.wrapping_mul(PRIME4_BYTES) >> (32 - h_bits)
}

/// Count matching bytes between `a[pos..]` and `b[0..]`, up to `limit`.
/// Matches C's `ZSTD_count`.
fn count_match(a: &[u8], a_pos: usize, b: &[u8], b_pos: usize, limit: usize) -> usize {
    let mut len = 0;
    while len < limit
        && a_pos + len < a.len()
        && b_pos + len < b.len()
        && a[a_pos + len] == b[b_pos + len]
    {
        len += 1;
    }
    len
}

/// A raw LZ77 sequence before entropy coding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawSequence {
    /// Number of literal bytes preceding this match.
    pub literal_length: u32,
    /// Match length (including the 4-byte minimum).
    pub match_length: u32,
    /// Byte distance to the match (1 = previous byte).
    pub offset: u32,
}

/// Literal buffer + sequence list produced by the match finder. The
/// block encoder consumes this to emit the literals section + sequences
/// section.
#[derive(Clone, Debug, Default)]
pub struct SeqStore {
    /// All literal bytes, concatenated.
    pub literals: Vec<u8>,
    /// Sequences (literal_length, match_length, offset) in order.
    pub sequences: Vec<RawSequence>,
    /// Repeat offsets, initialized to [1, 4, 8] per ZSTD spec. Updated
    /// by the match finder as matches are emitted.
    pub rep_offsets: [u32; REP_NUM],
}

impl SeqStore {
    /// Create a SeqStore with default repeat offsets [1, 4, 8].
    #[must_use]
    pub fn new() -> Self {
        Self {
            literals: Vec::new(),
            sequences: Vec::new(),
            rep_offsets: [1, 4, 8],
        }
    }

    /// Reset for a new block, carrying over repeat offsets.
    pub fn reset(&mut self, rep_offsets: [u32; REP_NUM]) {
        self.literals.clear();
        self.sequences.clear();
        self.rep_offsets = rep_offsets;
    }
}

/// Hash-table-based match state. Reused across blocks within a frame
/// to avoid re-allocating the hash table.
#[derive(Debug)]
pub struct MatchState {
    /// Hash table: `hash_table[h]` = last position with hash `h`, or 0.
    pub(crate) hash_table: Vec<u32>,
    pub(crate) hash_log: u32,
    /// Next position to insert into the hash table.
    pub(crate) next_to_update: u32,
}

impl MatchState {
    /// Create a match state for the given hash log.
    #[must_use]
    pub fn new(hash_log: u32) -> Self {
        let size = 1usize << hash_log;
        Self {
            hash_table: vec![0; size],
            hash_log,
            next_to_update: 0,
        }
    }

    /// Default hash log for level-1 fast mode. ZSTD uses ~hashLog 6-7
    /// for low levels, up to ~10 for level 3.
    #[must_use]
    pub fn default_hash_log() -> u32 {
        7
    }

    /// Resize the hash table for a different `hash_log`. Frees the old
    /// table only if the new size is larger (otherwise reuses the
    /// existing allocation and just tracks the new logical size).
    ///
    /// Called by [`ZstdCompressor`](crate::ZstdCompressor) when the
    /// input size or compression level changes between calls. After
    /// `resize_for`, callers should also call [`clear`](Self::clear) to
    /// zero out any stale entries.
    pub fn resize_for(&mut self, hash_log: u32) {
        if hash_log == self.hash_log {
            return;
        }
        let new_size = 1usize << hash_log;
        self.hash_table.resize(new_size, 0);
        self.hash_log = hash_log;
        self.next_to_update = 0;
    }

    /// Current hash log (table size = `1 << hash_log`).
    #[must_use]
    pub fn hash_log(&self) -> u32 {
        self.hash_log
    }

    /// Clear all hash entries. Call between blocks to prevent stale
    /// position references (positions are block-relative).
    pub fn clear(&mut self) {
        self.hash_table.fill(0);
        self.next_to_update = 0;
    }

    /// Seed the hash table with a dictionary prefix. Scans
    /// `buf[..prefix_len]` and inserts each 4-byte hash pointing to
    /// its absolute position within `buf`. Subsequent
    /// `compress_block_*_with_prefix` calls on slices of `buf` will
    /// find these dictionary positions as match candidates.
    ///
    /// Positions stored are absolute indices into `buf` (0-based from
    /// the start of the prefix). The block-level compressors must be
    /// told the `prefix_len` offset so their own positions stay
    /// consistent.
    pub(crate) fn seed_prefix(&mut self, buf: &[u8], prefix_len: usize) {
        if prefix_len < MIN_MATCH {
            return;
        }
        let limit = prefix_len - MIN_MATCH + 1;
        for pos in 0..limit {
            let h = hash4(buf, pos, self.hash_log);
            self.hash_table[h as usize] = pos as u32;
        }
        self.next_to_update = prefix_len as u32;
    }
}

/// Run the fast (greedy, single-match) parser over `src`. Appends
/// sequences and literals to `seq_store`. Returns the number of
/// trailing literal bytes (the last run that has no match).
///
/// Ported from C's `ZSTD_compressBlock_fast_generic` with `minMatch=4`,
/// simplified control flow. The `min_match` parameter controls the
/// minimum match length to accept.
pub fn compress_block_fast(
    src: &[u8],
    seq_store: &mut SeqStore,
    ms: &mut MatchState,
) -> usize {
    compress_block_with_min_match(src, seq_store, ms, MIN_MATCH)
}

/// Run the fast parser with a configurable minimum match length.
/// Lower `min_match` finds more (shorter) matches; higher values
/// skip short matches that cost more to encode than they save.
pub fn compress_block_with_min_match(
    src: &[u8],
    seq_store: &mut SeqStore,
    ms: &mut MatchState,
    min_match: usize,
) -> usize {
    if src.len() < min_match.max(4) + 1 {
        // Too short for any matches; emit all as literals.
        seq_store.literals.extend_from_slice(src);
        return src.len();
    }

    let h_bits = ms.hash_log;
    let mut anchor: usize = 0;
    let mut ip: usize = 0;
    let limit = src.len().saturating_sub(min_match.max(4));

    // Initial position skip: ZSTD skips the first position if it's
    // the start of the window.
    if ip == 0 {
        ip = 1;
    }

    while ip < limit {
        // Check for repcode match at current position.
        let rep0 = seq_store.rep_offsets[0];
        if ip > rep0 as usize && rep0 > 0 {
            if src[ip..ip + MIN_MATCH] == src[ip - rep0 as usize..ip - rep0 as usize + MIN_MATCH] {
                // Found a repcode match. Extend forward.
                let mut m_len = MIN_MATCH;
                m_len += count_match(src, ip + m_len, src, ip + m_len - rep0 as usize, limit + MIN_MATCH - ip - m_len);

                // Backward extension: absorb preceding literals.
                while ip > anchor && ip > rep0 as usize && src[ip - 1] == src[ip - 1 - rep0 as usize] {
                    ip -= 1;
                    m_len += 1;
                }

                // Acceptance check: only use matches that meet the
                // configured minimum match length.
                if m_len < min_match {
                    // Too short; treat as literal.
                    ip += 1;
                    continue;
                }

                let lit_len = (ip - anchor) as u32;
                seq_store.literals.extend_from_slice(&src[anchor..ip]);
                seq_store.sequences.push(RawSequence {
                    literal_length: lit_len,
                    match_length: m_len as u32,
                    offset: rep0,
                });

                // Rotate repeat offsets.
                rotate_reps(&mut seq_store.rep_offsets, rep0);

                // Insert positions into hash table, advance.
                insert_range(ms, src, ip, m_len);
                ip += m_len;
                anchor = ip;
                continue;
            }
        }

        // Hash current position.
        let h = hash4(src, ip, h_bits);
        let mut candidate = ms.hash_table[h as usize] as usize;

        // Update hash table with current position.
        ms.hash_table[h as usize] = ip as u32;

        // Check candidate: must be within window and match 4 bytes.
        if candidate > 0 && candidate < ip {
            let dist = ip - candidate;
            if dist < BLOCK_MAX_SIZE {
                // Verify 4-byte match.
                if src[ip..ip + MIN_MATCH] == src[candidate..candidate + MIN_MATCH] {
                    // Extend forward.
                    let mut m_len = MIN_MATCH;
                    m_len += count_match(src, ip + m_len, src, candidate + m_len, limit + MIN_MATCH - ip - m_len);

                    // Backward extension.
                    while ip > anchor && candidate > 0 && src[ip - 1] == src[candidate - 1] {
                        ip -= 1;
                        candidate -= 1;
                        m_len += 1;
                    }

                    // Acceptance check.
                    if m_len < min_match {
                        ip += 1;
                        continue;
                    }

                    let lit_len = (ip - anchor) as u32;
                    let offset = dist as u32;
                    seq_store.literals.extend_from_slice(&src[anchor..ip]);
                    seq_store.sequences.push(RawSequence {
                        literal_length: lit_len,
                        match_length: m_len as u32,
                        offset,
                    });

                    // Update repeat offsets.
                    rotate_reps(&mut seq_store.rep_offsets, offset);

                    // Insert positions into hash table, advance.
                    insert_range(ms, src, ip, m_len);
                    ip += m_len;
                    anchor = ip;
                    continue;
                }
            }
        }

        ip += 1;
    }

    // Emit trailing literals.
    if anchor < src.len() {
        seq_store.literals.extend_from_slice(&src[anchor..]);
    }
    src.len() - anchor
}

/// Lazy parser (look-ahead-1). At each position with a match, checks
/// if position+1 has a longer match. If so, emits a literal and defers.
///
/// Used for ZSTD levels 6-7 (Lazy strategy).
pub fn compress_block_lazy(
    src: &[u8],
    seq_store: &mut SeqStore,
    ms: &mut MatchState,
    min_match: usize,
) -> usize {
    if src.len() < min_match.max(4) + 1 {
        seq_store.literals.extend_from_slice(src);
        return src.len();
    }

    let h_bits = ms.hash_log;
    let mut anchor: usize = 0;
    let mut ip: usize = 1;
    let limit = src.len().saturating_sub(min_match.max(4));

    while ip < limit {
        let m1 = find_best_match(src, ip, h_bits, ms, min_match, limit);

        if let Some((dist1, len1)) = m1 {
            // Look ahead: check position ip+1 (read-only probe, no hash update).
            let m2 = if ip + 1 < limit {
                probe_match(src, ip + 1, h_bits, ms, min_match, limit)
            } else {
                None
            };

            let defer = match m2 {
                Some((_, len2)) => len2 > len1 + 1,
                None => false,
            };

            if defer {
                // Emit literal, try again at ip+1.
                ip += 1;
            } else {
                // Accept match at ip.
                let lit_len = (ip - anchor) as u32;
                seq_store.literals.extend_from_slice(&src[anchor..ip]);
                let offset = dist1 as u32;
                seq_store.sequences.push(RawSequence {
                    literal_length: lit_len,
                    match_length: len1 as u32,
                    offset,
                });
                rotate_reps(&mut seq_store.rep_offsets, offset);
                insert_range(ms, src, ip, len1);
                ip += len1;
                anchor = ip;
            }
        } else {
            ip += 1;
        }
    }

    if anchor < src.len() {
        seq_store.literals.extend_from_slice(&src[anchor..]);
    }
    src.len() - anchor
}

/// Lazy2 parser (look-ahead-2). Checks positions ip+1 AND ip+2 before
/// deciding. Used for ZSTD levels 8-12 (Lazy2 strategy) and as a
/// fallback for higher levels (Btopt/Btultra).
pub fn compress_block_lazy2(
    src: &[u8],
    seq_store: &mut SeqStore,
    ms: &mut MatchState,
    min_match: usize,
) -> usize {
    if src.len() < min_match.max(4) + 1 {
        seq_store.literals.extend_from_slice(src);
        return src.len();
    }

    let h_bits = ms.hash_log;
    let mut anchor: usize = 0;
    let mut ip: usize = 1;
    let limit = src.len().saturating_sub(min_match.max(4));

    while ip < limit {
        let m1 = find_best_match(src, ip, h_bits, ms, min_match, limit);

        if let Some((dist1, len1)) = m1 {
            // Look ahead 2 positions (read-only probes).
            let m2 = if ip + 1 < limit {
                probe_match(src, ip + 1, h_bits, ms, min_match, limit)
            } else {
                None
            };
            let m3 = if ip + 2 < limit {
                probe_match(src, ip + 2, h_bits, ms, min_match, limit)
            } else {
                None
            };

            // Defer if either look-ahead position has a better match.
            let defer1 = matches!(m2, Some((_, l)) if l > len1 + 1);
            let defer2 = matches!(m3, Some((_, l)) if l > len1 + 2);

            if defer1 || defer2 {
                ip += 1;
            } else {
                let lit_len = (ip - anchor) as u32;
                seq_store.literals.extend_from_slice(&src[anchor..ip]);
                let offset = dist1 as u32;
                seq_store.sequences.push(RawSequence {
                    literal_length: lit_len,
                    match_length: len1 as u32,
                    offset,
                });
                rotate_reps(&mut seq_store.rep_offsets, offset);
                insert_range(ms, src, ip, len1);
                ip += len1;
                anchor = ip;
            }
        } else {
            ip += 1;
        }
    }

    if anchor < src.len() {
        seq_store.literals.extend_from_slice(&src[anchor..]);
    }
    src.len() - anchor
}

/// Find the best match at `ip` using a single hash probe. Returns
/// `(distance, length)` or `None`.
///
/// This is the shared match-finding core used by all parsers. It:
/// 1. Hashes 4 bytes at `ip`.
/// 2. Updates the hash table.
/// 3. Checks the candidate for a valid match.
/// 4. Extends the match forward.
fn find_best_match(
    src: &[u8],
    ip: usize,
    h_bits: u32,
    ms: &mut MatchState,
    min_match: usize,
    limit: usize,
) -> Option<(usize, usize)> {
    if ip + MIN_MATCH > src.len() {
        return None;
    }

    // Hash probe.
    let h = hash4(src, ip, h_bits);
    let candidate = ms.hash_table[h as usize] as usize;
    ms.hash_table[h as usize] = ip as u32;

    if candidate == 0 || candidate >= ip {
        return None;
    }

    let dist = ip - candidate;
    if dist >= BLOCK_MAX_SIZE {
        return None;
    }

    if candidate + MIN_MATCH > src.len() {
        return None;
    }

    if src[ip..ip + MIN_MATCH] != src[candidate..candidate + MIN_MATCH] {
        return None;
    }

    let mut m_len = MIN_MATCH;
    m_len += count_match(src, ip + m_len, src, candidate + m_len, limit + MIN_MATCH - ip - m_len);

    if m_len >= min_match {
        Some((dist, m_len))
    } else {
        None
    }
}

/// Read-only match probe for look-ahead positions. Does NOT update
/// the hash table — used by lazy/lazy2 parsers to peek at future
/// positions without corrupting the hash state.
fn probe_match(
    src: &[u8],
    ip: usize,
    h_bits: u32,
    ms: &MatchState,
    min_match: usize,
    limit: usize,
) -> Option<(usize, usize)> {
    if ip + MIN_MATCH > src.len() {
        return None;
    }

    let h = hash4(src, ip, h_bits);
    let candidate = ms.hash_table[h as usize] as usize;

    if candidate == 0 || candidate >= ip {
        return None;
    }

    let dist = ip - candidate;
    if dist >= BLOCK_MAX_SIZE {
        return None;
    }

    if candidate + MIN_MATCH > src.len() {
        return None;
    }

    if src[ip..ip + MIN_MATCH] != src[candidate..candidate + MIN_MATCH] {
        return None;
    }

    let mut m_len = MIN_MATCH;
    m_len += count_match(src, ip + m_len, src, candidate + m_len, limit + MIN_MATCH - ip - m_len);

    if m_len >= min_match {
        Some((dist, m_len))
    } else {
        None
    }
}

/// Insert hash entries for positions `[start, start+len)` into the hash
/// table. Matches C's post-match hash filling. Only inserts every other
/// position (the C fast parser inserts positions `ip` and `ip+2` after
/// a match, for performance).
fn insert_range(ms: &mut MatchState, src: &[u8], start: usize, len: usize) {
    // The C fast parser inserts two hash entries after each match:
    // position `ip-2` (already passed) and `ip+2` (ahead of match end).
    // For simplicity, insert at `start` and `start + len - 2` (if valid).
    if start + MIN_MATCH <= src.len() {
        let h = hash4(src, start, ms.hash_log);
        ms.hash_table[h as usize] = start as u32;
    }
    if start + len >= 2 && start + len - 2 + MIN_MATCH <= src.len() {
        let pos = start + len - 2;
        let h = hash4(src, pos, ms.hash_log);
        ms.hash_table[h as usize] = pos as u32;
    }
}

/// Rotate the repeat offset array when a new offset is used.
/// The new offset becomes rep[0], the old rep[0] becomes rep[1], etc.
fn rotate_reps(reps: &mut [u32; REP_NUM], new_offset: u32) {
    if reps[0] == new_offset {
        // Same as rep[0] — no rotation needed.
    } else if reps[1] == new_offset {
        // Was rep[1]: swap rep[0] and rep[1].
        reps.swap(0, 1);
    } else if reps[2] == new_offset {
        // Was rep[2]: promote to rep[0], shift others down.
        reps[2] = reps[1];
        reps[1] = reps[0];
        reps[0] = new_offset;
    } else {
        // New offset: shift all down.
        reps[2] = reps[1];
        reps[1] = reps[0];
        reps[0] = new_offset;
    }
}

// ---------------------------------------------------------------------------
// Dictionary-prefix variants.
//
// These operate over `src = dict_content ++ plaintext`, with positions
// `[0, prefix_len)` being pre-seeded dictionary content (already
// inserted into the hash table via `MatchState::seed_prefix`).
//
// They iterate `[prefix_len, src.len())`, find matches that may point
// back into `[0, prefix_len)` (dictionary), and emit literals +
// sequences describing only the plaintext region. The decoder must
// prime its output window with `src[..prefix_len]` so back-references
// into the dictionary resolve.
// ---------------------------------------------------------------------------

/// Fast (greedy, single-match) parser over `src[prefix_len..]`,
/// treating `src[..prefix_len]` as pre-seeded dictionary content.
pub fn compress_block_fast_with_prefix(
    src: &[u8],
    prefix_len: usize,
    seq_store: &mut SeqStore,
    ms: &mut MatchState,
    min_match: usize,
) -> usize {
    let mm = min_match.max(4);
    if src.len() < prefix_len + mm + 1 {
        seq_store.literals.extend_from_slice(&src[prefix_len..]);
        return src.len() - prefix_len;
    }

    let h_bits = ms.hash_log;
    let mut anchor: usize = prefix_len;
    let mut ip: usize = if prefix_len == 0 { 1 } else { prefix_len };
    let limit = src.len().saturating_sub(mm);

    while ip < limit {
        // Check repcode.
        let rep0 = seq_store.rep_offsets[0];
        if rep0 > 0 && ip > rep0 as usize {
            if src[ip..ip + MIN_MATCH] == src[ip - rep0 as usize..ip - rep0 as usize + MIN_MATCH] {
                let mut m_len = MIN_MATCH;
                m_len += count_match(src, ip + m_len, src, ip + m_len - rep0 as usize, limit + MIN_MATCH - ip - m_len);

                while ip > anchor
                    && ip > rep0 as usize
                    && src[ip - 1] == src[ip - 1 - rep0 as usize]
                {
                    ip -= 1;
                    m_len += 1;
                }

                if m_len < min_match {
                    ip += 1;
                    continue;
                }

                let lit_len = (ip - anchor) as u32;
                seq_store.literals.extend_from_slice(&src[anchor..ip]);
                seq_store.sequences.push(RawSequence {
                    literal_length: lit_len,
                    match_length: m_len as u32,
                    offset: rep0,
                });
                rotate_reps(&mut seq_store.rep_offsets, rep0);
                insert_range_absolute(ms, src, ip, m_len);
                ip += m_len;
                anchor = ip;
                continue;
            }
        }

        // Hash + candidate lookup.
        let h = hash4(src, ip, h_bits);
        let mut candidate = ms.hash_table[h as usize] as usize;
        ms.hash_table[h as usize] = ip as u32;

        if candidate > 0 && candidate < ip {
            let dist = ip - candidate;
            if dist < BLOCK_MAX_SIZE
                && candidate + MIN_MATCH <= src.len()
                && src[ip..ip + MIN_MATCH] == src[candidate..candidate + MIN_MATCH]
            {
                let mut m_len = MIN_MATCH;
                m_len += count_match(src, ip + m_len, src, candidate + m_len, limit + MIN_MATCH - ip - m_len);

                while ip > anchor && candidate > 0 && src[ip - 1] == src[candidate - 1] {
                    ip -= 1;
                    candidate -= 1;
                    m_len += 1;
                }

                if m_len < min_match {
                    ip += 1;
                    continue;
                }

                let lit_len = (ip - anchor) as u32;
                let offset = dist as u32;
                seq_store.literals.extend_from_slice(&src[anchor..ip]);
                seq_store.sequences.push(RawSequence {
                    literal_length: lit_len,
                    match_length: m_len as u32,
                    offset,
                });
                rotate_reps(&mut seq_store.rep_offsets, offset);
                insert_range_absolute(ms, src, ip, m_len);
                ip += m_len;
                anchor = ip;
                continue;
            }
        }

        // Suppress unused-assignment warning when candidate is set but unused.
        let _ = candidate;
        ip += 1;
    }

    if anchor < src.len() {
        seq_store.literals.extend_from_slice(&src[anchor..]);
    }
    src.len() - anchor
}

/// Lazy parser with dictionary prefix. Same algorithm as
/// [`compress_block_lazy`] but operates over `src[prefix_len..]` with
/// `src[..prefix_len]` as pre-seeded dictionary content.
pub fn compress_block_lazy_with_prefix(
    src: &[u8],
    prefix_len: usize,
    seq_store: &mut SeqStore,
    ms: &mut MatchState,
    min_match: usize,
) -> usize {
    let mm = min_match.max(4);
    if src.len() < prefix_len + mm + 1 {
        seq_store.literals.extend_from_slice(&src[prefix_len..]);
        return src.len() - prefix_len;
    }

    let h_bits = ms.hash_log;
    let mut anchor: usize = prefix_len;
    let mut ip: usize = if prefix_len == 0 { 1 } else { prefix_len };
    let limit = src.len().saturating_sub(mm);

    while ip < limit {
        let m1 = find_best_match_absolute(src, ip, h_bits, ms, min_match, limit);

        if let Some((dist1, len1)) = m1 {
            let m2 = if ip + 1 < limit {
                probe_match_absolute(src, ip + 1, h_bits, ms, min_match, limit)
            } else {
                None
            };

            let defer = match m2 {
                Some((_, len2)) => len2 > len1 + 1,
                None => false,
            };

            if defer {
                ip += 1;
            } else {
                let lit_len = (ip - anchor) as u32;
                seq_store.literals.extend_from_slice(&src[anchor..ip]);
                let offset = dist1 as u32;
                seq_store.sequences.push(RawSequence {
                    literal_length: lit_len,
                    match_length: len1 as u32,
                    offset,
                });
                rotate_reps(&mut seq_store.rep_offsets, offset);
                insert_range_absolute(ms, src, ip, len1);
                ip += len1;
                anchor = ip;
            }
        } else {
            ip += 1;
        }
    }

    if anchor < src.len() {
        seq_store.literals.extend_from_slice(&src[anchor..]);
    }
    src.len() - anchor
}

/// Lazy2 parser with dictionary prefix.
pub fn compress_block_lazy2_with_prefix(
    src: &[u8],
    prefix_len: usize,
    seq_store: &mut SeqStore,
    ms: &mut MatchState,
    min_match: usize,
) -> usize {
    let mm = min_match.max(4);
    if src.len() < prefix_len + mm + 1 {
        seq_store.literals.extend_from_slice(&src[prefix_len..]);
        return src.len() - prefix_len;
    }

    let h_bits = ms.hash_log;
    let mut anchor: usize = prefix_len;
    let mut ip: usize = if prefix_len == 0 { 1 } else { prefix_len };
    let limit = src.len().saturating_sub(mm);

    while ip < limit {
        let m1 = find_best_match_absolute(src, ip, h_bits, ms, min_match, limit);

        if let Some((dist1, len1)) = m1 {
            let m2 = if ip + 1 < limit {
                probe_match_absolute(src, ip + 1, h_bits, ms, min_match, limit)
            } else {
                None
            };
            let m3 = if ip + 2 < limit {
                probe_match_absolute(src, ip + 2, h_bits, ms, min_match, limit)
            } else {
                None
            };

            let defer1 = matches!(m2, Some((_, l)) if l > len1 + 1);
            let defer2 = matches!(m3, Some((_, l)) if l > len1 + 2);

            if defer1 || defer2 {
                ip += 1;
            } else {
                let lit_len = (ip - anchor) as u32;
                seq_store.literals.extend_from_slice(&src[anchor..ip]);
                let offset = dist1 as u32;
                seq_store.sequences.push(RawSequence {
                    literal_length: lit_len,
                    match_length: len1 as u32,
                    offset,
                });
                rotate_reps(&mut seq_store.rep_offsets, offset);
                insert_range_absolute(ms, src, ip, len1);
                ip += len1;
                anchor = ip;
            }
        } else {
            ip += 1;
        }
    }

    if anchor < src.len() {
        seq_store.literals.extend_from_slice(&src[anchor..]);
    }
    src.len() - anchor
}

/// Find the best match at absolute position `ip` in `src`. Updates
/// the hash table. Same as `find_best_match` but uses absolute
/// positions (so candidates from the dictionary prefix are visible).
fn find_best_match_absolute(
    src: &[u8],
    ip: usize,
    h_bits: u32,
    ms: &mut MatchState,
    min_match: usize,
    limit: usize,
) -> Option<(usize, usize)> {
    if ip + MIN_MATCH > src.len() {
        return None;
    }
    let h = hash4(src, ip, h_bits);
    let candidate = ms.hash_table[h as usize] as usize;
    ms.hash_table[h as usize] = ip as u32;

    if candidate == 0 || candidate >= ip {
        return None;
    }
    let dist = ip - candidate;
    if dist >= BLOCK_MAX_SIZE {
        return None;
    }
    if candidate + MIN_MATCH > src.len() {
        return None;
    }
    if src[ip..ip + MIN_MATCH] != src[candidate..candidate + MIN_MATCH] {
        return None;
    }
    let mut m_len = MIN_MATCH;
    m_len += count_match(src, ip + m_len, src, candidate + m_len, limit + MIN_MATCH - ip - m_len);
    if m_len >= min_match {
        Some((dist, m_len))
    } else {
        None
    }
}

/// Read-only match probe using absolute positions.
fn probe_match_absolute(
    src: &[u8],
    ip: usize,
    h_bits: u32,
    ms: &MatchState,
    min_match: usize,
    limit: usize,
) -> Option<(usize, usize)> {
    if ip + MIN_MATCH > src.len() {
        return None;
    }
    let h = hash4(src, ip, h_bits);
    let candidate = ms.hash_table[h as usize] as usize;
    if candidate == 0 || candidate >= ip {
        return None;
    }
    let dist = ip - candidate;
    if dist >= BLOCK_MAX_SIZE {
        return None;
    }
    if candidate + MIN_MATCH > src.len() {
        return None;
    }
    if src[ip..ip + MIN_MATCH] != src[candidate..candidate + MIN_MATCH] {
        return None;
    }
    let mut m_len = MIN_MATCH;
    m_len += count_match(src, ip + m_len, src, candidate + m_len, limit + MIN_MATCH - ip - m_len);
    if m_len >= min_match {
        Some((dist, m_len))
    } else {
        None
    }
}

/// Insert hash entries for positions `[start, start+len)` — same as
/// `insert_range` but the function is already position-absolute.
fn insert_range_absolute(ms: &mut MatchState, src: &[u8], start: usize, len: usize) {
    if start + MIN_MATCH <= src.len() {
        let h = hash4(src, start, ms.hash_log);
        ms.hash_table[h as usize] = start as u32;
    }
    if start + len >= 2 && start + len - 2 + MIN_MATCH <= src.len() {
        let pos = start + len - 2;
        let h = hash4(src, pos, ms.hash_log);
        ms.hash_table[h as usize] = pos as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_produces_no_sequences() {
        let mut ss = SeqStore::new();
        let mut ms = MatchState::new(7);
        let trailing = compress_block_fast(b"", &mut ss, &mut ms);
        assert_eq!(trailing, 0);
        assert!(ss.sequences.is_empty());
        assert!(ss.literals.is_empty());
    }

    #[test]
    fn short_input_all_literals() {
        let mut ss = SeqStore::new();
        let mut ms = MatchState::new(7);
        let input = b"abc";
        let trailing = compress_block_fast(input, &mut ss, &mut ms);
        assert_eq!(trailing, 3);
        assert!(ss.sequences.is_empty());
        assert_eq!(ss.literals, input);
    }

    #[test]
    fn repetitive_input_finds_matches() {
        // "abcdefghabcdefghabcdefgh" — 8-byte pattern repeated 3 times.
        let mut ss = SeqStore::new();
        let mut ms = MatchState::new(7);
        let input = b"abcdefghabcdefghabcdefgh";
        let _ = compress_block_fast(input, &mut ss, &mut ms);
        // Should find at least one match.
        assert!(!ss.sequences.is_empty(), "expected at least one match");
        // Reconstruct the original from literals + sequences.
        let mut reconstructed = Vec::new();
        let mut lit_pos = 0;
        for seq in &ss.sequences {
            reconstructed.extend_from_slice(&ss.literals[lit_pos..lit_pos + seq.literal_length as usize]);
            lit_pos += seq.literal_length as usize;
            let off = seq.offset as usize;
            let ml = seq.match_length as usize;
            let start = reconstructed.len() - off;
            for i in 0..ml {
                let b = reconstructed[start + i];
                reconstructed.push(b);
            }
        }
        // Trailing literals.
        reconstructed.extend_from_slice(&ss.literals[lit_pos..]);
        assert_eq!(&reconstructed[..], input);
    }

    #[test]
    fn hash4_is_deterministic() {
        let data = b"hello world";
        let h1 = hash4(data, 0, 7);
        let h2 = hash4(data, 0, 7);
        assert_eq!(h1, h2);
        // Different positions should usually hash differently.
        let h3 = hash4(data, 1, 7);
        assert_ne!(h1, h3);
    }

    #[test]
    fn repcode_rotation_basics() {
        let mut reps = [1u32, 4, 8];
        rotate_reps(&mut reps, 4); // existing rep[1]
        assert_eq!(reps, [4, 1, 8]); // swap 0 and 1

        let mut reps = [1u32, 4, 8];
        rotate_reps(&mut reps, 8); // existing rep[2]
        assert_eq!(reps, [8, 1, 4]); // promote, shift

        let mut reps = [1u32, 4, 8];
        rotate_reps(&mut reps, 100); // new offset
        assert_eq!(reps, [100, 1, 4]); // new at 0, shift down
    }
}
