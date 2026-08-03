# 103 — Multi-byte FSE decoder (decoupled from differential harness)

**Priority:** Medium — unblocks TODO 84
**Source:** LimniFS proposal `omnizip-proposals/multibyte-fse-unblock.md`
**Status:** ⏳ Pending — implementation plan landed; PR TBD

## Problem

TODO 84 proposes a level-2 FSE decode table that processes 2–4 input
bytes per state transition, for ~30% throughput gain. It is **blocked**
on TODO 87 (differential harness) with this rationale:

> The current FSE decoder has subtle correctness corner cases...
> Multi-byte decode doubles the surface area for bugs; we need the
> differential harness to validate.

LimniFS agrees that differential testing is essential. But the
multi-byte decoder can be **landed first** as a parallel
implementation, validated against the *existing* scalar decoder
(rather than the C reference), and only enabled by default once the
differential harness confirms byte-identical output.

## Proposed sequencing

### Phase 1 — Parallel implementation (unblocked)

Implement `interleaved::decode` alongside `fse::decode`. Validate
exclusively against the scalar decoder:

```rust
#[cfg(test)]
mod differential_tests {
    fn check_against_scalar(input: &[u8], table: &Table) {
        let scalar = fse::decode(input, table);
        let interleaved = interleaved::decode(input, table);
        assert_eq!(scalar, interleaved);
    }

    #[test]
    fn random_inputs_match_scalar() { /* proptest 10^6 inputs */ }

    #[test]
    fn real_corpus_matches_scalar() { /* Calgary + Enwik8 + Silesia */ }
}
```

The scalar decoder is the oracle. Multi-byte is correct iff it agrees
with scalar on every input the test suite covers.

Dispatch stays scalar-by-default:

```rust
pub fn decode(input: &[u8], table: &Table) -> Vec<u8> {
    #[cfg(feature = "multibyte-fse")]
    if input.len() >= MULTIBYTE_THRESHOLD {
        return interleaved::decode(input, table);
    }
    scalar::decode(input, table)
}
```

### Phase 2 — Enable by default (needs differential harness)

Once TODO 87 lands, run the differential harness against the C
reference. If multi-byte agrees with C, flip the default. If not,
fix bugs first.

## Algorithm (per ACM 2024 paper)

The standard rANS/FSE state update is:

```text
state = (state / denominator) * total_symbols + state % denominator
```

The multi-byte variant packs two state updates into one 64-bit
operation, halving the per-symbol overhead. Reference:
*Efficient and Portable ANS Encoding for Multi-Byte Integer
Sequences* (ACM 2024).

### Implementation sketch

```rust
const INTERLEAVE_FACTOR: usize = 2; // 2-byte batching (start with this)

pub fn decode(input: &[u8], table: &Table) -> Vec<u8> {
    let mut states = [init_state(input, 0), init_state(input, 8)];
    let mut out = Vec::with_capacity(table.expected_output_len);
    let mut bit_pos = 16; // start after the two state values
    while bit_pos < input.len() * 8 {
        for i in 0..INTERLEAVE_FACTOR {
            let sym = table.lookup(states[i]);
            out.push(sym);
            states[i] = renormalize(states[i], input, &mut bit_pos);
        }
    }
    out
}
```

## Acceptance criteria (Phase 1)

- [ ] `interleaved::decode` exists with a level-2 lookup table.
- [ ] Differential tests against scalar pass on:
  - Calgary corpus
  - Silesia chunks
  - Enwik8 chunks
  - 10⁶ random inputs (proptest)
- [ ] Throughput improvement ≥ 20% on ZSTD level-19 Enwik8 decode.
- [ ] Behind a `multibyte-fse` feature flag (default off).

## Effort estimate

5–7 days:
- 3 days: level-2 table generator + decode loop
- 2 days: proptest differential harness against scalar
- 1 day: Silesia/Enwik8 benchmark
- 1 day: code review + cleanup

## Related

- omnizip-rs TODO 84
- ACM (2024). *Efficient and Portable ANS Encoding for Multi-Byte
  Integer Sequences.* https://dl.acm.org/doi/10.1145/3712285.3759825
- Kosolobov (2022). *Efficiency of ANS Entropy Encoders.*
  https://arxiv.org/pdf/2201.02514
