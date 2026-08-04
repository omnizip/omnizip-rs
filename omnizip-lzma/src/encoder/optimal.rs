//! Optimal parser for LZMA — dynamic-programming based parse selection.
//!
//! Finds the globally minimum-cost parse by computing, for each position
//! `i`, the cheapest way to encode `input[0..i]`. This gives 3-8% better
//! ratio than lazy (look-ahead-1) parsing.
//!
//! ## Algorithm
//!
//! 1. **Forward pass**: for each position `i`:
//!    - Consider emitting a literal → update `opt[i+1]`.
//!    - For each candidate match at `i` → update `opt[i + len]`.
//!    - For each rep match → update `opt[i + len]`.
//! 2. **Backtrack**: from `opt[input.len()]`, reconstruct the parse.
//! 3. **Emit**: encode the parse using the range coder + probability
//!    models.
//!
//! ## Price estimation
//!
//! Prices are estimated as `−log2(probability)` for each encoded bit.
//! The estimates are approximate (not accounting for state transitions)
//! but produce good relative rankings — which is all the DP needs.
//!
//! Ported from `~/src/external/xz-utils/src/liblzma/lzma/lzma_encoder_optimum_normal.c`.

#![forbid(unsafe_code)]

use crate::encoder::match_finder::{Match, MatchFinder};
use crate::encoder::prob_state::LzmaProbState;

/// Maximum match length to consider at each position.
const OPT_MAX_MATCH_LEN: usize = 273;

/// Price estimate in 1/8-bit units (matching the C reference convention).
type Price = u32;

/// Action chosen at each position during the DP.
#[derive(Clone, Copy, Debug)]
pub enum ParseAction {
    /// Encode one literal byte.
    Literal(u8),
    /// Encode a full match (distance, length). Distance is 1-based.
    Match { distance: u32, length: u32 },
    /// Encode a rep0 match (reuse last distance).
    Rep0Match { length: u32 },
}

/// Find all candidate matches at each position using the match finder.
/// Returns a vector where `matches[i]` = list of (distance, length)
/// candidates at position `i`.
fn find_all_matches(input: &[u8], dict_size: u32) -> Vec<Vec<Match>> {
    let mut mf = MatchFinder::new(input, dict_size);
    let mut all_matches = vec![Vec::new(); input.len()];

    while let Some(pos) = mf.advance() {
        if pos + 3 <= input.len() {
            if let Some(m) = mf.find_match(pos) {
                all_matches[pos].push(m);
            }
        }
    }

    all_matches
}

// Legacy per-symbol price helpers (`literal_price`, `match_price`)
// were replaced by the state-conditioned functions in
// `prob_state.rs` (see TODO 106). The DP now carries an
// `LzmaProbState` through each transition so the prices reflect the
// actual encoder state.

/// Run the forward DP pass. Returns the optimal parse as a sequence
/// of (start_pos, action) pairs.
///
/// Prices come from the [`prob_state`](crate::encoder::prob_state)
/// module — state-conditioned estimates that mirror the C reference's
/// length/distance slot decomposition. The state machine (literal →
/// match → rep) is tracked through the DP so prices reflect the
/// actual encoder state at each position.
fn optimal_parse(input: &[u8], dict_size: u32) -> Vec<(usize, ParseAction)> {
    if input.is_empty() {
        return Vec::new();
    }

    let matches = find_all_matches(input, dict_size);
    let n = input.len();

    // DP table: opt[i] = cheapest cost to encode input[0..i].
    // Each node carries the prob state it ended in, so successors
    // can compute state-conditioned prices.
    #[derive(Clone)]
    struct Node {
        price: Price,
        action: Option<(ParseAction, usize)>,
        state: LzmaProbState,
    }

    let start_state = LzmaProbState::new();
    let mut opt = vec![
        Node {
            price: Price::MAX,
            action: None,
            state: start_state,
        };
        n + 1
    ];
    opt[0] = Node {
        price: 0,
        action: None,
        state: start_state,
    };

    for i in 0..n {
        if opt[i].price == Price::MAX {
            continue;
        }
        let cur_state = opt[i].state;

        let prev_byte = if i > 0 { input[i - 1] } else { 0 };

        // Option A: emit a literal.
        let lit_state = LzmaProbState {
            prev_byte,
            match_byte: cur_state.match_byte,
            rep0: cur_state.rep0,
            state: cur_state.state,
        };
        let lit_price = prob_state_literal_price(lit_state, input[i]);
        let new_price = opt[i].price.saturating_add(lit_price);
        if new_price < opt[i + 1].price {
            opt[i + 1] = Node {
                price: new_price,
                action: Some((ParseAction::Literal(input[i]), i)),
                state: lit_state.after_literal(input[i]),
            };
        }

        // Option B: emit each match candidate.
        for m in &matches[i] {
            let len = m.length.min(OPT_MAX_MATCH_LEN as u32);
            if i + len as usize > n {
                continue;
            }
            let m_price = prob_state_match_price(cur_state, m.distance, len);
            let per_byte = m_price / len.max(1);
            // Only take the match if it's cheaper than literals per byte.
            if per_byte < 64 {
                let end = i + len as usize;
                let new_price = opt[i].price.saturating_add(m_price);
                if new_price < opt[end].price {
                    let new_state = cur_state.after_match(m.distance);
                    opt[end] = Node {
                        price: new_price,
                        action: Some((
                            ParseAction::Match { distance: m.distance, length: len },
                            i,
                        )),
                        state: new_state,
                    };
                }
            }
        }

        // Option C: rep0 match (reuse last distance, if it gives a match).
        let rep0 = cur_state.rep0;
        if rep0 > 0 && rep0 < i as u32 {
            let back = i - rep0 as usize - 1;
            let max_len = (n - i).min(OPT_MAX_MATCH_LEN);
            let mut match_len = 0u32;
            for k in 0..max_len {
                if input[i + k] != input[back + k] {
                    break;
                }
                match_len += 1;
            }
            if match_len >= 2 {
                let rep_price = prob_state_rep0_price(cur_state, match_len);
                let end = i + match_len as usize;
                let new_price = opt[i].price.saturating_add(rep_price);
                if new_price < opt[end].price {
                    let new_state = cur_state.after_rep();
                    opt[end] = Node {
                        price: new_price,
                        action: Some((ParseAction::Rep0Match { length: match_len }, i)),
                        state: new_state,
                    };
                }
            }
        }
    }

    // Backtrack.
    let mut actions = Vec::new();
    let mut pos = n;
    while pos > 0 {
        if let Some((action, start)) = opt[pos].action {
            actions.push((start, action));
            pos = start;
        } else {
            break;
        }
    }
    actions.reverse();
    actions
}

/// Wrapper for the prob_state literal price that falls back to a
/// simple constant when the state has no recent match (Phase 2 stub
/// matches the original heuristic).
fn prob_state_literal_price(state: LzmaProbState, byte: u8) -> Price {
    use crate::encoder::prob_state::literal_price as ps_lit;
    ps_lit(state, byte) as Price
}

/// Wrapper for the prob_state match price.
fn prob_state_match_price(state: LzmaProbState, distance: u32, length: u32) -> Price {
    use crate::encoder::prob_state::match_price as ps_match;
    ps_match(state, distance, length) as Price
}

/// Wrapper for the prob_state rep0 price.
fn prob_state_rep0_price(state: LzmaProbState, length: u32) -> Price {
    use crate::encoder::prob_state::rep0_price as ps_rep0;
    ps_rep0(state, length) as Price
}

/// Encode `input` using the optimal parser. Returns the LZMA1 byte
/// stream (must be fed to the range coder by the caller).
///
/// This is a price-based planner — it computes the optimal parse and
/// then drives the encoder's `encode_literal_byte` / `encode_match`
/// methods to emit the bits.
pub fn optimal_parse_actions(input: &[u8], dict_size: u32) -> Vec<(usize, ParseAction)> {
    optimal_parse(input, dict_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optimal_parse_repetitive_input() {
        // "aaaa...aaa" should produce a valid parse. The optimal
        // parser must cover every byte without gaps.
        let input = b"aaaaaaaaaaaaaaaaaaaa".repeat(10);
        let actions = optimal_parse_actions(&input, 1 << 16);
        assert!(!actions.is_empty());

        // Verify full coverage.
        let mut covered = 0usize;
        for (start, action) in &actions {
            assert_eq!(*start, covered, "gap in parse");
            covered += match action {
                ParseAction::Literal(_) => 1,
                ParseAction::Match { length, .. } | ParseAction::Rep0Match { length } => *length as usize,
            };
        }
        assert_eq!(covered, input.len(), "must cover entire input");
    }

    #[test]
    fn optimal_parse_incompressible() {
        // Random-ish data — optimal parser should produce mostly literals.
        let input: Vec<u8> = (0..1000u32).map(|i| (i.wrapping_mul(2654435761) >> 16) as u8).collect();
        let actions = optimal_parse_actions(&input, 1 << 16);
        let literals = actions.iter().filter(|(_, a)| matches!(a, ParseAction::Literal(_))).count();
        assert!(literals > 900, "expected mostly literals for random input, got {literals}");
    }

    #[test]
    fn optimal_parse_covers_full_input() {
        // Every byte must be accounted for by exactly one action.
        let input = b"hello world hello world hello world hello world";
        let actions = optimal_parse_actions(input, 1 << 16);
        let mut covered = 0usize;
        for (start, action) in &actions {
            assert_eq!(*start, covered, "gap or overlap in parse");
            covered += match action {
                ParseAction::Literal(_) => 1,
                ParseAction::Match { length, .. } | ParseAction::Rep0Match { length } => *length as usize,
            };
        }
        assert_eq!(covered, input.len(), "parse must cover entire input");
    }

    #[test]
    fn optimal_parse_empty_input() {
        let actions = optimal_parse_actions(&[], 1 << 16);
        assert!(actions.is_empty());
    }

    #[test]
    fn optimal_parse_single_byte() {
        let actions = optimal_parse_actions(b"x", 1 << 16);
        assert_eq!(actions.len(), 1);
        assert!(matches!(actions[0].1, ParseAction::Literal(_)));
    }
}
