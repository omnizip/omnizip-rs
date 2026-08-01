//! LZMA state machine — 12 states tracking match/literal history.
//!
//! Ported line-by-line from `omnizip/lib/omnizip/algorithms/lzma/state.rb`
//! (MIT, Ribose Inc.). The transition tables match the XZ Utils reference
//! (`xz/src/liblzma/lzma/lzma_decoder.c`).
//!
//! ## Design
//!
//! The state is a single byte (0–11). It transitions on every symbol
//! (literal, match, rep-match, short-rep) via const-array lookup. The
//! state selects which probability model the range coder uses for the
//! next decision — see [`crate::constants::NUM_STATES`].

#![forbid(unsafe_code)]

/// Total number of LZMA states.
pub const NUM_STATES: usize = 12;

/// Transition table for `on_literal()`. After encoding a literal byte,
/// the new state is `LIT_STATES[old_state]`.
///
/// Rationale: consecutive literals shift toward state 0 (the "fresh"
/// context). After a match, literals shift through states 4–6 (the
/// "just-matched" context). After a rep, literals go to state 9.
pub const LIT_STATES: [u8; NUM_STATES] = [0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 4, 5];

/// Transition table for `on_match()`. After encoding a full match (new
/// distance), the new state is `MATCH_STATES[old_state]`.
pub const MATCH_STATES: [u8; NUM_STATES] = [7, 7, 7, 7, 7, 7, 7, 10, 10, 10, 10, 10];

/// Transition table for `on_rep()`. After encoding a rep-match (reusing
/// the last distance), the new state is `REP_STATES[old_state]`.
pub const REP_STATES: [u8; NUM_STATES] = [8, 8, 8, 8, 8, 8, 8, 11, 11, 11, 11, 11];

/// Transition table for `on_short_rep()`. A short rep (length 0,
/// effectively "repeat the last byte once") transitions differently.
pub const SHORT_REP_STATES: [u8; NUM_STATES] = [9, 9, 9, 9, 9, 9, 9, 11, 11, 11, 11, 11];

/// The LZMA state machine. Wraps a `u8` index (0–11).
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash)]
pub struct LzmaState(u8);

impl LzmaState {
    /// Construct a state at the given index. Panics if `index >= 12`.
    ///
    /// # Panics
    ///
    /// Panics if `index` ≥ [`NUM_STATES`].
    #[must_use]
    pub const fn new(index: u8) -> Self {
        assert!((index as usize) < NUM_STATES, "LZMA state must be 0..11");
        Self(index)
    }

    /// The initial state (0 — fresh, no recent history).
    #[must_use]
    pub const fn initial() -> Self {
        Self(0)
    }

    /// The raw state index (0–11).
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self.0
    }

    /// Transition after a literal byte.
    pub fn on_literal(&mut self) {
        self.0 = LIT_STATES[usize::from(self.0)];
    }

    /// Transition after a full match (new distance).
    pub fn on_match(&mut self) {
        self.0 = MATCH_STATES[usize::from(self.0)];
    }

    /// Transition after a rep-match (reused distance).
    pub fn on_rep(&mut self) {
        self.0 = REP_STATES[usize::from(self.0)];
    }

    /// Transition after a short rep (length-0 match at the last distance).
    pub fn on_short_rep(&mut self) {
        self.0 = SHORT_REP_STATES[usize::from(self.0)];
    }

    /// States 0–6 are "literal context": the encoder recently emitted
    /// literals, not deep in a match sequence.
    #[must_use]
    pub const fn is_literal_context(self) -> bool {
        self.0 < 7
    }

    /// States 7–11 are "match context": a match or rep was recently
    /// emitted. The literal coder uses a special "matched literal" path.
    #[must_use]
    pub const fn is_match_context(self) -> bool {
        self.0 >= 7
    }

    /// States 8–11 are "rep context": a rep-match was recently emitted.
    /// `IsRep[state]` selects rep-vs-new-distance.
    #[must_use]
    pub const fn is_rep_context(self) -> bool {
        self.0 >= 7
    }

    /// Reset to the initial state (0).
    pub fn reset(&mut self) {
        self.0 = 0;
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_is_zero() {
        let s = LzmaState::initial();
        assert_eq!(s.as_u8(), 0);
    }

    #[test]
    fn literal_transitions_match_reference() {
        for (input, expected) in LIT_STATES.iter().enumerate() {
            let mut s = LzmaState::new(input as u8);
            s.on_literal();
            assert_eq!(
                s.as_u8(),
                *expected,
                "LIT_STATES[{input}] should be {expected}"
            );
        }
    }

    #[test]
    fn match_transitions_match_reference() {
        for (input, expected) in MATCH_STATES.iter().enumerate() {
            let mut s = LzmaState::new(input as u8);
            s.on_match();
            assert_eq!(s.as_u8(), *expected, "MATCH_STATES[{input}]");
        }
    }

    #[test]
    fn rep_transitions_match_reference() {
        for (input, expected) in REP_STATES.iter().enumerate() {
            let mut s = LzmaState::new(input as u8);
            s.on_rep();
            assert_eq!(s.as_u8(), *expected, "REP_STATES[{input}]");
        }
    }

    #[test]
    fn short_rep_transitions_match_reference() {
        for (input, expected) in SHORT_REP_STATES.iter().enumerate() {
            let mut s = LzmaState::new(input as u8);
            s.on_short_rep();
            assert_eq!(s.as_u8(), *expected, "SHORT_REP_STATES[{input}]");
        }
    }

    #[test]
    fn literal_then_match_sequence() {
        let mut s = LzmaState::initial();
        s.on_literal(); // 0 → 0
        s.on_literal(); // 0 → 0
        s.on_match(); // 0 → 7
        s.on_literal(); // 7 → 4
        s.on_rep(); // 4 → 8
        assert_eq!(s.as_u8(), 8);
    }

    #[test]
    fn context_queries() {
        let s0 = LzmaState::new(0);
        assert!(s0.is_literal_context());
        assert!(!s0.is_match_context());

        let s7 = LzmaState::new(7);
        assert!(!s7.is_literal_context());
        assert!(s7.is_match_context());
        assert!(s7.is_rep_context());

        let s11 = LzmaState::new(11);
        assert!(s11.is_match_context());
        assert!(s11.is_rep_context());
    }

    #[test]
    fn reset_returns_to_zero() {
        let mut s = LzmaState::new(11);
        s.reset();
        assert_eq!(s.as_u8(), 0);
    }

    #[test]
    #[should_panic(expected = "LZMA state must be 0..11")]
    fn rejects_out_of_range() {
        let _ = LzmaState::new(12);
    }
}
