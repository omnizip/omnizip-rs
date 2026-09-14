# Task 35 — brotli: distance-split cost at q5 text (designed)

Status: designed (2026-09-14; measurement done, implementation next)

## Finding

The reference-port distance split (`split_byte_vector` on dist
symbols, 3 iterations) at q5 text costs **53% of encode** on
rustsrc/words (the #1 and #2 hot cells after task 34) for mixed value:

| file | size (old split) | Δ | time ratio |
|---|---|---|---|
| rustsrc | +1,022B (+0.23%) | **0.47×** |
| words | −424B (−0.06%, WINS) | **0.62×** |
| csv2m | +6,421B (+3.20% — NEEDS the split) | 0.42× |
| plists | +642B (+0.49%) | 0.57× |
| dbdump | +103B | 1.00× |
| install | +16B | ~1.0 |
| icons | +275B | ~1.0 |

csv2m's periodic structure rides the distance split heavily (6.4KB);
words is neutral-to-better without it. A simple OFF default regresses
csv2m — not shippable as-is.

## Design (the cost, not the on/off)

The split's cost is `cluster_histograms`' O(m²) priority-queue merge
(the reference's HistogramPair list). Cheaper forms to try, in order:

1. **Iteration cap at q5**: 3 → 1 iteration (the reference's own
   default for non-max quality is lower).
2. **Histogram sampling**: cluster a strided sample of histograms,
   then assign the rest to the nearest centroid (upstream's own
   `MAX_NUM_HISTOGRAMS` pre-reduction).
3. **Cost-gate**: run the full split only when the distance-symbol
   entropy spread is wide (cheap pre-histogram test — csv2m-class
   structure detects as multi-modal; words-class is narrow).

Target: rustsrc/words q5 T −30..−50% with csv2m held within +0.2%.


## Update (post task-34 release, load rising)

Also tested: a content-class gate (csv2m/plists are Structured and
benefit; words/rustsrc are Text with mixed ±) — but plists loses
+0.49% without the split, so the class split is not clean either.
The lever is the split's own cost (fewer sampled histograms /
sampling before clustering / a cheap multi-modality pre-gate), not an
on/off. Load rose past 25 mid-investigation; parked with data.