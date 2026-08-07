# 172: Brotli Decoder — Full RFC 7932 Support

## Priority: P3

## Status: DONE — 100% differential pass rate (220/220).

## What landed (2026-08-07)

The decoder handles ALL RFC 7932 features needed for non-trivial brotli streams.

### Major fixes (in chronological order)

- **ISLAST=1 metablock-header fix** (PR #135) — bit-position drift bug.
- **Context map reader** (PR #136) — `read_context_map` + inverse MTF.
- **Flat 2^15 lookup Huffman decoder** (PR #153) — replaces broken bit-by-bit walker.
- **O(1) table-based Huffman decode** (PR #152) — ported from upstream.
- **NSYM=3 simple form depths** (PR #157) — 1+2+2 not 2+2+2.
- **kCmdLut regeneration** (PR #158) — `const fn` mirroring upstream algorithm.
- **Static dictionary** (PR #158) — 122784-byte embedded data + 121 transforms.
- **LZ77 dist_rb write-back** (PR #158) — was missing, caused ring buffer drift.
- **prev_code_len semantics** (PR #158) — only update on sym ≠ 0.
- **K_DISTANCE_CONTEXT_BITS = 2** (PR #160) — was 6 (copy-paste from literal).
- **NTREES dispatch ordering** (PR #160) — NTREES_D read after lit cm, not before.
- **NSYM=4 tree_select=1** (PR #162) — mixed 1+1+2+2 layout via direct table build.

## Differential test matrix

220 test cases: 20 diverse inputs × 11 quality levels (q=1..11),
encoded via reference `brotli -q N`, decoded via omnizip-rs.

**220/220 pass (100%).**
