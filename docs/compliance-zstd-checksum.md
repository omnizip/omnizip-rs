# ZSTD frame checksum — consumed but not verified

## Status

**Open.** The decoder reads the 4-byte checksum at the end of a
checksummed frame and discards it.

## Affected code

`omnizip-zstd/src/decoder.rs` — `decode_frame`, line `remaining = &remaining[4..];`

## What RFC 8878 says

RFC 8878 §3.1.1 (Frame_Header.Descriptor):

> Bit 2 is the Content_Checksum flag. When set, a 32-bit checksum
> follows the last block of the frame, computed over the
> uncompressed output using the XXH32 algorithm with seed 0.

The decoder MUST verify the checksum and report a mismatch as a
corruption error.

## What the C reference does

The C reference (`lib/decompress/zstd_decompress.c`) computes XXH32
over the output as it is produced, then compares against the
trailing 4 bytes. A mismatch returns
`ERROR(corruption_detected)`.

## What the Rust port does

The Rust port's `decode_frame` consumes the 4 trailing bytes when
the checksum flag is set, but does not compute or compare anything:

```rust
if header.has_checksum() {
    if remaining.len() < 4 {
        return Err(ZstdError::Corrupt {
            reason: "truncated frame checksum".into(),
        });
    }
    // TODO: real XXHash32 verification.
    remaining = &remaining[4..];
}
```

## What the Ruby port does (bug)

The Ruby computes a hash, but uses a DJB2 polynomial
(`hash = hash * 33 + byte`) instead of XXH32. The check always
fails (or always passes on short inputs that hash to zero). See
`../omnizip/BUGREPORT.06-xxhash32-wrong-algorithm.md`.

## Why the divergence exists

XXH32 is a moderately complex algorithm (four-lane stripe mixing
with five magic primes, ~80 LOC for a single-shot implementation).
It was deferred in favour of landing the decode pipeline. The
checksum is a defense-in-depth mechanism; skipping verification
does not affect correctness of the decoded bytes themselves, only
the decoder's ability to detect corruption.

## Impact

Corrupted frames that would be caught by checksum verification are
not caught. The decoded output may be silently wrong if the input
was damaged.

## Reconciliation plan

1. Implement XXH32 (single-shot variant is sufficient; the decoder
   does not need streaming). The algorithm is documented at
   <https://github.com/Cyan4973/xxHash>.
2. Compute XXH32 over the frame output.
3. Compare against the trailing 4 bytes (little-endian u32).
4. Return `ZstdError::Corrupt` on mismatch.

Estimated effort: half a day.
