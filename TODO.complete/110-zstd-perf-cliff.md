# TODO 110: ZSTD encoder O(N²) perf cliff on text ≥ 8KB

## Status

**FIXED.** Root cause found and fixed in PR #85.

## Original symptom

`ZstdCodec::compress` hung (took >60 seconds) on text inputs of 8 KiB
and larger, at every level (1, 3, 9, 19, 22). On 4 KiB of identical
content it completed in milliseconds; on 8 KiB it stalled indefinitely.

## Root cause

Infinite loop in `compress_block_with_min_match` (and the prefix-aware
variant `compress_block_fast_with_prefix`). The backward-extension
loops decremented `ip` *before* the acceptance check:

```rust
// Old buggy code
while ip > anchor && ip > rep0 && src[ip - 1] == src[ip - 1 - rep0] {
    ip -= 1;       // ← mutates ip
    m_len += 1;
}
if m_len < min_match {
    ip += 1;       // ← undoes only the last decrement
    continue;      // ← returns to top of while with same ip
}
```

When the data was periodic with period `rep0` at position `ip`, the
4-byte repcode match kept succeeding but the extended match fell just
below `min_match` (default 7 at level 1). Each iteration: backward-extend
1, accept-check fails, `ip += 1; continue;` → loop forever at the same ip.

The bench's text generator produced 8 distinct short words repeated
randomly, creating many periodic regions that triggered this.

## Fix

Compute the backward extension as a *count* (`back`) without mutating
`ip`. Apply the count once, after the acceptance check passes:

```rust
let mut back = 0usize;
while ip > anchor + back && ip > rep0 + back
    && src[ip - 1 - back] == src[ip - 1 - rep0 - back]
{
    back += 1;
}
m_len += back;
if m_len < min_match {
    ip += 1;       // ← advances past original ip
    continue;
}
ip -= back;        // ← apply back-extension atomically
```

Applied the same fix to the candidate-match path (both functions) and
to the dict-prefix variant (`compress_block_fast_with_prefix`).

## Verified

After fix: 8 KiB text input compresses in **295 µs** at level 1 (was
"infinite"). Bench harness now runs ZSTD at every level on 64 KiB text
without timeouts:

```
zstd- 1:  11.3 MB/s  ratio=3.18×
zstd- 3:  47.3 MB/s  ratio=2.79×
zstd- 9:  47.6 MB/s  ratio=2.79×
zstd-19:   9.1 MB/s  ratio=2.89×
```

All 174 existing ZSTD tests still pass.
