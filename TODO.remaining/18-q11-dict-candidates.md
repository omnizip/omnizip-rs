# 18 — q11 collect-level dictionary candidates

- **Priority:** LOW (measured gain so far: 0.005–0.07%)
- **Depends on:** nothing open
- **Status:** done 2026-09-05 — implemented, measured, shipped env-gated default-OFF

## Background

0.21.49 shipped a q11-only block in `zopfli_hq::collect_matches` that
probed static-dictionary candidates into the shared H10 match list.
It never fired: both `dict_hash::find_match(data, i, u32::MAX)` and
the follow-up `dictionary_lookup(.., u32::MAX)` used `u32::MAX` as the
distance base — the addition `max_distance + 1 + address` overflowed
(debug panic, red CI on main) and wrapped in release, where the lookup
then computed a negative address and rejected every candidate. The
block was removed to restore main (byte-identical to the 0.21.49
release output, where the feature was inert).

## Why activation is not a one-line fix

With the base corrected to `min(mlen_offset + i, MAX_BACKWARD)`
(matching btopt's working `dict_at`), three bug classes surfaced:

1. **Chunk-tail overrun** — candidates lacked the `pos + len <= n`
   guard that `dict_at` has; a word at the chunk tail extends past
   `n` and panics the literal re-walk (rustsrc/bin2 class).
2. **Distance-cache pollution (the hard one)** — the HQ DP's rep
   relaxation pulls node distances into the StartPosQueue's distance
   cache, then treats them as in-window copy sources
   (`data[pos + l] == data[prev + l]`). A dictionary distance is not
   an in-window source, so the DP records "copies" at dictionary
   distances with lengths the dictionary word does not have
   (`cpy != declen` everywhere in pass 2) — the command walk desyncs
   and reference decoders reject or mis-decode the stream
   (arial/rfc REF-FAIL class).
3. Upstream's HQ zopfli feeds NO dictionary matches into this DP at
   all — the reference's q11 has no precedent to port; this is
   net-new design.

## Design sketch (for the next attempt)

- Make the DP dictionary-aware at the node level, like btopt's
  `CODE_DICT` back-pointer code: a dict node records the word
  (address + length), is excluded from `compute_distance_cache`
  pushes, and cannot be extended by rep/±delta probes.
- Reuse the working `dict_at` contract: distance computed against the
  decoder's clamped output position, length from
  `dictionary_lookup`'s output (`tmp.len()`), never the finder's own
  `tl`, and always guarded by `pos + len <= n`.
- Gate: `best_len < 16` (dict_at's gate) rather than 24.
- Validate on: rustsrc, bin2 (panics), arial, rfc (REF-FAIL),
  words (−34B win), plus the 10-file corpus sweep + REF-DECODE on
  every cell.

## Acceptance

- Every q11 corpus cell REF-DECODE valid; no walk desyncs at any
  `BROTLI_DICT_CHAIN` depth.
- Net ratio win across the corpus (not just single files).
- Debug + release tests green.


## Resolution (2026-09-05) — implemented via dict-aware DP; env-gated OFF

The design sketch above is now real code, validated end-to-end:

- `build_dict_at` (btopt's lookup-validated builder) is shared into
  `parse_hq_with` (mlen_offset threaded through); the DP relaxes each
  position's candidate as a `CODE_DICT_SHORT` (17) node — length-
  preserving only, priced at the exact top-bucket wire shape
  (`long_dist_symbol(d)` + 24 extra bits).
- `compute_distance_cache` never pushes dictionary distances (the
  PR #465 root cause), so rep relaxation cannot treat them as
  in-window sources. The backtrack's existing `len_code`-vs-`copy_len`
  Command shape is exact for tl == wl.
- Gate: reference rule — candidates only where the position's best
  match is below 4 bytes.

**Measured (with the gate, ungated by `BROTLI_Q11_DICT=1`):**
words −41, rustsrc −63, arial −422, five cells flat, all REF-OK,
deterministic, MT-identical — but the real-corpus `plists.json` cell
**+373** (per-transition pricing cannot predict total-emission
tree-shape shifts; the contest guards hq-vs-btopt, not new-vs-old).
Net ≈ 0.01% across the corpus.

**Shipped default OFF** (`BROTLI_Q11_DICT=1` restores; the
`BROTLI_MID_DICT` precedent): default output stays byte-identical to
main on every cell. The always-on variant needs a third
exact-emission contest candidate (hq-with-dict vs hq-without) at
~+40% q11 cost — documented here if the ratio ever matters.
