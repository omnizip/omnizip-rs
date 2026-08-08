# 210 — ZSTD Optimal FSE Table Selection

- **Priority:** P1 (10% ratio win, highest ZSTD improvement)
- **Crate:** `omnizip-zstd`
- **Depends on:** none
- **Estimated effort:** 2 weeks

## Goal

Implement optimal FSE table selection for the sequence section
(literal lengths, match lengths, offsets). Currently uses default
tableLog=6 for all three alphabets. The C reference searches the
parameter space to find the tableLog that minimizes bit cost.

## Background

ZSTD uses FSE (Finite State Entropy) to compress the three sequence
symbol alphabets:

1. **Literal lengths** (35 symbols): how many literals to insert
2. **Offsets** (32 symbols): match distance
3. **Match lengths** (53 symbols): match length

Each FSE table has a `tableLog` parameter (accuracyLog) that controls
the trade-off between table description size and per-symbol cost.
Higher tableLog = more accurate probability model but larger table
header.

Current state: tableLog=6 for all three, hardcoded. The C reference
tries tableLog 5–8 and picks the cheapest.

## Scope

1. **Cost estimation** (1 week): given a histogram and a candidate
   tableLog, estimate the total FSE bit cost (header + payload).

2. **Table search** (3 days): for each alphabet, try all valid
   tableLog values and pick the cheapest.

3. **Table construction** (2 days): build the FSE table from the
   optimal tableLog and histogram.

## Acceptance criteria

- [ ] tableLog is no longer hardcoded to 6
- [ ] Cost estimation within 5% of actual FSE size
- [ ] Ratio improvement ≥ 5% vs fixed tableLog=6
- [ ] `zstd -d` accepts output at all levels
- [ ] No encode speed regression > 20%

## Implementation plan

### New module: `omnizip-zstd/src/encoder/fse_optimizer.rs`

```rust
pub fn optimal_fse_table(histogram: &[u32], max_accuracy_log: u32) -> FseTable {
    let mut best = None;
    for log in 5..=max_accuracy_log {
        let table = build_fse_table(histogram, log);
        let cost = estimate_fse_cost(&table, histogram);
        match best {
            None => best = Some((cost, table)),
            Some((bc, _)) if cost < bc => best = Some((cost, table)),
            _ => {}
        }
    }
    best.unwrap().1
}

fn estimate_fse_cost(table: &FseTable, histogram: &[u32]) -> u32 {
    let header_cost = table.header_size_bits();
    let payload_cost: u32 = histogram.iter().enumerate()
        .map(|(sym, &count)| count * table.symbol_cost(sym))
        .sum();
    header_cost + payload_cost
}
```

### Integration with block encoder

In `encoder/block.rs:write_block`, replace fixed tableLog with
`optimal_fse_table()` calls for each of the three alphabets.

## Test plan

- Unit test: cost estimation matches actual FSE size within 5%
- Unit test: tableLog selection is optimal for known histograms
- Integration: ratio improvement ≥ 5% on Silesia corpus
- Integration: `zstd -d` accepts output
- Benchmark: encode speed regression < 20%

## References

- RFC 8478 §4.1.1 (FSE table format)
- C reference: `zstd/compress/zstd_compress_internal.h:ZSTD_selectEncodingType`
- Our encoder: `encoder/block.rs:write_sequences_section`
- Our decoder: `fse/from_stream.rs:FseDecoder` (already handles any
  tableLog)
