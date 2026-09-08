# 08 — fits brotli q5 >=1MiB time (was scored I=16.8)

- **Status:** closed 2026-09-08 — CELL WAS MIS-SCORED

The v3-v6 boards compared our WALL time vs the reference's USER
time. The ours-side "rest" batch ran under box load-246 and silently
overrode the good first-batch rows: fits brotli q5 was scored
T=17.9x from a 2.126s wall measurement; the true values are
0.2325s (first batch) / 0.263s (user-time re-measure) vs ref 0.111s
user — **T ~ 1.9-2.4x, I ~ 1.8-2.3: never a whack cell.**

Fixes that shipped during the investigation anyway:
- v0.21.69: sub-1MiB bank hasher (task 06).
- v0.21.70: reference sparse-search heuristic ported to the greedy
  tier (fits q5 -28% time under load, 2,641,545 -> 2,640,769 bytes).

Methodology fix (v7 board onward): ours measured as USER CPU time
(RUNS=10 amortized, /usr/bin/time -p), same basis as the reference
side. The board file documents per-row load conditions.
