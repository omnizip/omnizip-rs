//! Port of xz's `lzma_encoder_optimum_fast.c` — the parse used by
//! `LZMA_MODE_FAST` (low preset levels).
//!
//! One `decide()` call returns the command at the current position:
//! a literal, a rep match (reuse one of the four recent distances),
//! or a new-distance match. The caller owns the match finder (it
//! must have inserted every position `<= pos`) and drives emission.
//!
//! ## Determinism
//!
//! No RNG, no hash-map iteration; every table walk is sequential.
//! Byte-identical output for identical input + `nice_len`.

use crate::encoder::match_finder::MatchFinder;

const SENTINEL: u32 = u32::MAX;
const MATCH_LEN_MAX: u32 = 273;
const REPS: usize = 4;
const H3_SIZE: usize = 1 << 16;

/// `change_pair` from `lzma_encoder_optimum_fast.c`: prefer the shorter
/// candidate when its distance is dramatically smaller.
#[inline]
const fn change_pair(small_dist: u32, big_dist: u32) -> bool {
    (big_dist >> 7) > small_dist
}

/// One parse decision — xz's `(back_res, len_res)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastCommand {
    Literal,
    /// Rep match at `index` 0..=3 (0 = most recent distance).
    /// `length` ≥ 2.
    Rep {
        index: u32,
        length: u32,
    },
    /// New-distance match. `distance` is 1-based.
    Match {
        distance: u32,
        length: u32,
    },
}

impl FastCommand {
    /// Positions consumed — xz's `len_res`.
    #[must_use]
    pub const fn consumed(&self) -> u32 {
        match *self {
            Self::Literal => 1,
            Self::Rep { length, .. } | Self::Match { length, .. } => length,
        }
    }
}

/// Result of one `decide()` call: the command plus the highest input
/// position the parse inserted while deciding (the lookahead at
/// `pos + 1` inserts one extra; skip-covered positions count up to
/// `pos + length - 1`).
#[derive(Debug, Clone, Copy)]
pub struct FastDecision {
    pub command: FastCommand,
    /// Highest inserted position (inclusive) after the call.
    pub inserted_through: usize,
}

/// Carry state across positions:
///
/// - `head2` / `head3` — the hash-2 / hash-3 tables of xz's HC4
///   finder (`lzma_mf_hc4_find`'s delta2/delta3 probes). Exact-key
///   (2 bytes) and verified-key (3 bytes) respectively, so ladder
///   entries are always true matches.
/// - `pending` — the cached one-byte-lookahead search (xz's
///   `read_ahead == 1` + `longest_match_length` carry).
///
/// Ladder entries are `(length, distance)` with **0-based** distance,
/// matching xz's `lzma_match` convention.
pub struct FastParseState {
    head2: Vec<u32>,
    head3: Vec<u32>,
    pending: Option<Pending>,
    scratch: Vec<(u32, u32)>,
}

struct Pending {
    len_main: u32,
    ladder: Vec<(u32, u32)>,
}

impl Default for FastParseState {
    fn default() -> Self {
        Self::new()
    }
}

/// Common prefix length of `input[pos..]` and `input[pos - dist..]`,
/// starting the scan at `from` (bytes below `from` are already
/// verified) and capped at `limit`. Mirrors `lzma_memcmplen`.
fn common_prefix(input: &[u8], pos: usize, dist: usize, from: u32, limit: u32) -> u32 {
    let max = limit as usize;
    let mut len = from as usize;
    while len + 8 <= max {
        let a = u64::from_le_bytes(input[pos + len..pos + len + 8].try_into().unwrap());
        let b = u64::from_le_bytes(
            input[pos + len - dist..pos + len - dist + 8]
                .try_into()
                .unwrap(),
        );
        if a == b {
            len += 8;
        } else {
            return (len + ((a ^ b).trailing_zeros() >> 3) as usize) as u32;
        }
    }
    while len < max && input[pos + len] == input[pos + len - dist] {
        len += 1;
    }
    len as u32
}

#[inline]
fn key3(input: &[u8], pos: usize) -> usize {
    let v = u32::from_le_bytes([input[pos], input[pos + 1], input[pos + 2], 0]);
    ((v.wrapping_mul(0x9E37_79B1) >> 16) & 0xFFFF) as usize
}

impl FastParseState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            head2: vec![SENTINEL; 1 << 16],
            head3: vec![SENTINEL; H3_SIZE],
            pending: None,
            scratch: Vec::new(),
        }
    }

    /// Write `pos` into the hash-2 / hash-3 tables. xz does this for
    /// every position the match finder passes (find and skip alike),
    /// gated on at least 4 bytes being available (`hc4_skip`'s
    /// `mf_avail(mf) < 4` guard).
    fn insert_side(&mut self, input: &[u8], pos: usize) {
        if pos + 4 > input.len() {
            return;
        }
        let key2 = u16::from_ne_bytes([input[pos], input[pos + 1]]) as usize;
        self.head2[key2] = pos as u32;
        self.head3[key3(input, pos)] = pos as u32;
    }

    /// Full match search at `pos` — the port of `lzma_mf_hc4_find` +
    /// `lzma_mf_find`'s nice-length extension. Returns the longest
    /// match length; `ladder` holds the improving-length candidates
    /// as `(length, 0-based distance)`.
    fn ladder_at(
        &mut self,
        input: &[u8],
        mf: &MatchFinder,
        pos: usize,
        len_limit: u32,
        ladder: &mut Vec<(u32, u32)>,
    ) -> u32 {
        ladder.clear();
        let mut len_best = 1u32;

        if pos + 4 <= input.len() && len_limit >= 4 {
            // delta2 probe: most recent position with the same 2 bytes.
            let key2 = u16::from_ne_bytes([input[pos], input[pos + 1]]) as usize;
            let mut delta2 = u32::MAX;
            let c2 = self.head2[key2];
            if c2 != SENTINEL {
                let d = (pos as u32).wrapping_sub(c2);
                let du = d as usize;
                if du >= 1 && du <= pos && d <= mf.max_distance() && input[pos - du] == input[pos] {
                    delta2 = d;
                    ladder.push((2, d - 1));
                    len_best = 2;
                }
            }

            // delta3 probe: most recent position with the same 3 bytes
            // (hash-verified, not key-exact).
            let c3 = self.head3[key3(input, pos)];
            if c3 != SENTINEL {
                let d3 = (pos as u32).wrapping_sub(c3);
                let d3u = d3 as usize;
                if d3 != delta2
                    && d3u >= 1
                    && d3u <= pos
                    && d3 <= mf.max_distance()
                    && input[pos - d3u] == input[pos]
                    && input[pos + 1 - d3u] == input[pos + 1]
                    && input[pos + 2 - d3u] == input[pos + 2]
                {
                    delta2 = d3;
                    ladder.push((3, d3 - 1));
                    len_best = 3;
                }
            }

            // Extend the last probe entry to its true length.
            if !ladder.is_empty() {
                let du = delta2 as usize;
                let l = common_prefix(input, pos, du, len_best, len_limit);
                if let Some(last) = ladder.last_mut() {
                    last.0 = l;
                }
                len_best = l;
            }
        }

        // Chain walk (xz's `if (len_best < 3) len_best = 3;` +
        // hc_find_func), appending strictly-longer candidates.
        let seed = len_best.max(3);
        self.scratch.clear();
        mf.walk_chain_ladder(pos, len_limit, seed, &mut self.scratch);
        for &(len, dist) in &self.scratch {
            ladder.push((len, dist - 1));
        }
        if ladder.is_empty() {
            return 0;
        }
        let mut len_main = ladder[ladder.len() - 1].0;

        // lzma_mf_find: when the longest hit the nice-length cap,
        // extend it to the true length (up to MATCH_LEN_MAX / input
        // end) — the returned length may exceed len_limit, the
        // ladder entry keeps the capped length.
        if len_main == len_limit {
            let du = ladder[ladder.len() - 1].1 as usize + 1;
            let true_limit = ((input.len() - pos) as u32).min(MATCH_LEN_MAX);
            len_main = common_prefix(input, pos, du, len_main, true_limit);
        }
        len_main
    }

    /// Skip `count` positions starting at `from`: insert each into the
    /// match finder and the side tables (xz's `mf_skip` → `hc4_skip`).
    fn skip(&mut self, mf: &mut MatchFinder, input: &[u8], from: usize, count: u32) {
        for p in from..from + count as usize {
            mf.advance();
            self.insert_side(input, p);
        }
    }

    /// Decide the command at `pos`.
    ///
    /// Contract: every position `<= pos` is inserted in `mf`.
    /// [`FastDecision::inserted_through`] reports the new highest
    /// inserted position.
    // Kept whole to mirror lzma_lzma_optimum_fast's control flow
    // branch-for-branch.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn decide(
        &mut self,
        input: &[u8],
        mf: &mut MatchFinder,
        pos: usize,
        reps: [u32; REPS],
        nice_len: u32,
    ) -> FastDecision {
        let nice_len = if nice_len == 0 || nice_len > MATCH_LEN_MAX {
            MATCH_LEN_MAX
        } else {
            nice_len
        };
        let n = input.len();
        let avail = n - pos;
        let buf_avail = (avail as u32).min(MATCH_LEN_MAX);

        if buf_avail < 2 {
            self.insert_side(input, pos);
            return FastDecision {
                command: FastCommand::Literal,
                inserted_through: pos,
            };
        }

        // mf_find at pos — or the cached lookahead result.
        let (mut len_main, mut ladder) = if let Some(p) = self.pending.take() {
            (p.len_main, p.ladder)
        } else {
            let mut ladder = Vec::new();
            let l = self.ladder_at(input, mf, pos, buf_avail.min(nice_len), &mut ladder);
            self.insert_side(input, pos);
            (l, ladder)
        };

        // Rep scan: direct comparison at each of the four recent
        // distances (reps are 0-based; wire distance = rep + 1).
        let mut rep_len = 0u32;
        let mut rep_index = 0u32;
        for (i, &r) in reps.iter().enumerate() {
            let dist = r as usize + 1;
            if dist > pos {
                continue;
            }
            if input[pos] != input[pos - dist] || input[pos + 1] != input[pos + 1 - dist] {
                continue;
            }
            let len = common_prefix(input, pos, dist, 2, buf_avail);
            if len >= nice_len {
                self.skip(mf, input, pos + 1, len - 1);
                return FastDecision {
                    command: FastCommand::Rep {
                        index: i as u32,
                        length: len,
                    },
                    inserted_through: pos + len as usize - 1,
                };
            }
            if len > rep_len {
                rep_index = i as u32;
                rep_len = len;
            }
        }

        // Long enough main match: take immediately.
        if len_main >= nice_len {
            let dist0 = ladder[ladder.len() - 1].1;
            self.skip(mf, input, pos + 1, len_main - 1);
            return FastDecision {
                command: FastCommand::Match {
                    distance: dist0 + 1,
                    length: len_main,
                },
                inserted_through: pos + len_main as usize - 1,
            };
        }

        // change_pair shortening: walk the ladder down while each
        // step's distance shrinks dramatically.
        let mut back_main = 0u32;
        if len_main >= 2 {
            back_main = ladder[ladder.len() - 1].1;
            while ladder.len() > 1 && len_main == ladder[ladder.len() - 2].0 + 1 {
                if !change_pair(ladder[ladder.len() - 2].1, back_main) {
                    break;
                }
                ladder.pop();
                len_main = ladder[ladder.len() - 1].0;
                back_main = ladder[ladder.len() - 1].1;
            }
            if len_main == 2 && back_main >= 0x80 {
                len_main = 1;
            }
        }

        // Rep preference: reps are far cheaper to encode.
        if rep_len >= 2
            && (rep_len + 1 >= len_main
                || (rep_len + 2 >= len_main && back_main > (1 << 9))
                || (rep_len + 3 >= len_main && back_main > (1 << 15)))
        {
            self.skip(mf, input, pos + 1, rep_len - 1);
            return FastDecision {
                command: FastCommand::Rep {
                    index: rep_index,
                    length: rep_len,
                },
                inserted_through: pos + rep_len as usize - 1,
            };
        }

        if len_main < 2 || buf_avail <= 2 {
            return FastDecision {
                command: FastCommand::Literal,
                inserted_through: pos,
            };
        }

        // Lookahead one byte: if the match starting at pos + 1 is
        // better, emit a literal here.
        mf.advance();
        let mut ladder2 = Vec::new();
        let len_limit2 = (buf_avail - 1).min(nice_len);
        let len2 = self.ladder_at(input, mf, pos + 1, len_limit2, &mut ladder2);
        self.insert_side(input, pos + 1);

        if len2 >= 2 {
            let new_dist = ladder2[ladder2.len() - 1].1;
            if (len2 >= len_main && new_dist < back_main)
                || (len2 == len_main + 1 && !change_pair(back_main, new_dist))
                || (len2 > len_main + 1)
                || (len2 + 1 >= len_main && len_main >= 3 && change_pair(new_dist, back_main))
            {
                self.pending = Some(Pending {
                    len_main: len2,
                    ladder: ladder2,
                });
                return FastDecision {
                    command: FastCommand::Literal,
                    inserted_through: pos + 1,
                };
            }
        }

        // If a rep match starts at pos + 1 and is nearly as long as
        // the main match here, prefer the future rep (literal now).
        let limit = (len_main - 1).max(2) as usize;
        if pos + 1 + limit <= n {
            let mut rep_next = false;
            for &r in &reps {
                let dist = r as usize + 1;
                if dist > pos + 1 {
                    continue;
                }
                if input[pos + 1..pos + 1 + limit] == input[pos + 1 - dist..pos + 1 - dist + limit]
                {
                    rep_next = true;
                    break;
                }
            }
            if rep_next {
                self.pending = Some(Pending {
                    len_main: len2,
                    ladder: ladder2,
                });
                return FastDecision {
                    command: FastCommand::Literal,
                    inserted_through: pos + 1,
                };
            }
        }

        // Take the main match. pos + 1 is already inserted.
        self.skip(mf, input, pos + 2, len_main - 2);
        FastDecision {
            command: FastCommand::Match {
                distance: back_main + 1,
                length: len_main,
            },
            inserted_through: pos + len_main as usize - 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::match_finder::new_lzma_match_finder;

    fn commands_for(input: &[u8], nice: u32) -> Vec<FastCommand> {
        let mut mf = new_lzma_match_finder(input, 1 << 16);
        mf.set_max_chain_length(8);
        let mut st = FastParseState::new();
        let mut reps = [0u32; 4];
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < input.len() {
            while mf.position() <= pos {
                mf.advance();
            }
            let d = st.decide(input, &mut mf, pos, reps, nice);
            out.push(d.command);
            match d.command {
                FastCommand::Match { distance, .. } => {
                    reps[3] = reps[2];
                    reps[2] = reps[1];
                    reps[1] = reps[0];
                    reps[0] = distance - 1;
                }
                FastCommand::Rep { index, .. } => {
                    if index > 0 {
                        let d2 = reps[index as usize];
                        reps[3] = reps[2];
                        reps[2] = reps[1];
                        reps[1] = reps[0];
                        reps[0] = d2;
                    }
                }
                FastCommand::Literal => {}
            }
            pos += d.command.consumed() as usize;
        }
        out
    }

    #[test]
    fn periodic_data_uses_rep_matches() {
        let data = b"alpha,beta,gamma,42\n".repeat(200);
        let cmds = commands_for(&data, 32);
        let reps = cmds
            .iter()
            .filter(|c| matches!(c, FastCommand::Rep { .. }))
            .count();
        assert!(reps > 10, "expected rep matches, got {reps} in {cmds:?}");
    }

    #[test]
    fn commands_are_deterministic() {
        let data: Vec<u8> = (0..20_000u32)
            .map(|i| (i.wrapping_mul(7).wrapping_add(i >> 3)) as u8)
            .collect();
        assert_eq!(commands_for(&data, 64), commands_for(&data, 64));
    }

    #[test]
    fn literal_only_at_stream_start() {
        // Nothing to match before position 3; the first command on
        // non-trivial input must not reference a nonexistent position.
        let data = b"abcdefgh";
        let cmds = commands_for(data, 4);
        assert_eq!(cmds[0], FastCommand::Literal);
    }
}
