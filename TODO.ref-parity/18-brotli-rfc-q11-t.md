# Task 18 — brotli: rfc q11 fresh T measurement (the last >4 cell)

Status: closed (measured + accepted, 2026-09-12)

## Measurement

The v18–v20 boards carried rfc.txt brotli q11 at T=4.3 from the v16 time
sweep (2026-09-10) — which predates v0.21.79 (tree-RLE port), .80 (dict in
base hq parse), .81 (splitter fusion), and .82 (binary b-skip); three of
those four touch the q11 path. Fresh 50x-loop serial measurement
(2026-09-12, both bases ≥2s):

- ours: 8.00s / 50 = **160 ms/encode**
- ref (homebrew brotli 1.2.0): 2.00s / 50 = **40 ms/encode**
- **T = 4.0** (was carried 4.3) → I = 4.0 × 1.014 = **4.1**

So the cell is real, not a noise artifact, but slightly better than
carried.

## Disposition: ACCEPT

rfc.txt is a 25 KB dense-text input — exactly the class the full q11
contest (hq, bt, hq+dict, iter, conditional split-b, treecap) was built
for; every candidate's win in this class was measured when the contest was
assembled (task 04; rfc's winner IS the dict candidate +
treecap: hqdict 56,050 → treecap 53,418). The 120 ms absolute overhead on
a 160 ms encode is the insurance premium that holds the S side at 1.014
(and smaller on other dense-text files). No candidate gate exists that
preserves the size wins while skipping the parses on this class — the
binary-class b-skip (task 16) used the clean Binary/Text split; within
dense text the winners genuinely vary per file.

Reopen condition: a cheap predictor that separates which dense-text
candidate wins before parsing (none known; the A/B/C literal-assignment
internals were inspected in task 16 and offer no pre-parse signal).
