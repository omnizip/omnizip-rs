# Task 46 — "do std::simd stabilization": built, measured twice, disconfirmed

Status: closed (2026-09-16)

## What was built (all reverted; recipe recorded)

1. build.rs availability probe (resolves the compiler cargo actually
   uses via $RUSTUP_HOME/toolchains/$RUSTUP_TOOLCHAIN — a plain PATH
   lookup hits a shadowing system rustc); success emits
   cargo:rustc-cfg=zsimd; #![cfg_attr(zsimd, feature(portable_simd))]
   at the crate root; ZSIMD=0 forces off.
2. Fair A/B protocol: the SAME nightly tree built twice (cfg on/off) —
   cross-toolchain comparison is invalid (1.100-nightly vs 1.98-stable
   differs by more than the kernel).
3. count_abs ports, TWO designs, both byte-identical by construction
   (8-44/44 corpus cells at L1/L6/L12/L19).

## The measurements (nightly 1.100.0, loop canon)

| kernel | dbdump L19 | csv2m L19 | words L19 |
|---|---|---|---|
| v1: to_array first-nonzero | +13% | +21% | +5% |
| v2: register XOR + OR-chain fast path | +48% | +10% | +18% |

(load 14-36 both rounds; direction consistent across designs.)
Both slower than scalar — matches end within 8-32 bytes, the wide
step rarely completes, and lane construction exceeds the compares
saved. Third+fourth confirmation of task 40's manual [u64;4] result.

## The corrected premise (supersedes tasks 30/36/45)

"SIMD closes the ~2x per-op gap" is DISCONFIRMED: the 21 floor cells'
hot loops (q1 CreateCommands, ZSTD_fast, lazy chain walk, btopt DP)
are sequential dependent processes, and the C references are scalar —
the gap is bounds-checks/codegen shape, not missing vectorization.
std::simd stabilization will NOT cross these cells. The canary stays
(it is free) but the reopen premise is now: an algorithmic
breakthrough, not a toolchain event. The board's remaining 21 cells
are terminal for this codebase's safe-Rust constraints.

TRAPS logged: `rustup run nightly cargo` does not repath children
(homebrew rustc shadowed everything — the "nightly build" was
stable-built until RUSTC was pinned to the toolchain binary);
Mask<u64> is not a MaskElement; reduce_or unavailable (OR-chain
instead); compound `{ grep } || { patch }` blocks skip the patch when
grep succeeds; `rm` is interactive in this shell — rm -f always.
