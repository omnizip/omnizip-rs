# 01 — LZMA / LZMA2 / XZ requirements

## Functional

| ID | Requirement |
|---|---|
| L-F01 | Decode every `.xz` file produced by reference `xz` at levels 0–9. |
| L-F02 | Decode every `.lzma` (alone) file produced by `lzma` CLI. |
| L-F03 | Decode every `.lz` (lzip) file produced by `lzip`. |
| L-F04 | Encode at levels 0–9; output round-trips through reference `xz -d`. |
| L-F05 | Encode output is byte-identical to omnizip Ruby at matching level + parameters. |
| L-F06 | Support BCJ filters (x86, ARM, ARM64, IA64, PPC, SPARC) as XZ filter-chain steps. |
| L-F07 | Support delta filter (configurable distance 1–256). |

## Non-functional

| ID | Requirement | Target |
|---|---|---|
| L-N01 | Decode throughput | ≥ 50 MB/s single-core on Apple M1 |
| L-N02 | Encode throughput at level 2 | ≥ 10 MB/s single-core |
| L-N03 | Encode throughput at level 6 | ≥ 3 MB/s single-core |
| L-N04 | Memory at level 6 (64 MB dictionary) | ≤ 128 MB peak |
| L-N05 | Cold start (first decode) | ≤ 100 µs overhead vs steady-state |
| L-N06 | Ratio vs reference `xz -9` on Silesia | within 3% |
| L-N07 | Ratio vs reference `xz -6` on Silesia | within 5% |

## Error handling

| ID | Requirement |
|---|---|
| L-E01 | Reject `dict_size` > 1 GiB with `OmnizipError::Corrupt`. |
| L-E02 | Reject invalid properties byte (`lc + lp > 4`) with `OmnizipError::Corrupt`. |
| L-E03 | Detect range-coder underrun (code > range) as `OmnizipError::Corrupt`. |
| L-E04 | Detect CRC64 mismatch in XZ footer as `OmnizipError::Corrupt`. |
| L-E05 | Detect CRC32 mismatch in XZ header/flags as `OmnizipError::Corrupt`. |
| L-E06 | Return `OmnizipError::LengthMismatch` if decompressed output ≠ `expected_len`. |

## API

| ID | Requirement |
|---|---|
| L-A01 | `lzma2_compress(plaintext, level)` returns `Result<Vec<u8>, LzmaError>`. |
| L-A02 | `lzma2_decompress(compressed, expected_len)` returns `Result<Vec<u8>, LzmaError>`. |
| L-A03 | `LzmaLevel` is a newtype wrapping `u8`, clamped to 0–9. |
| L-A04 | Level 6 is the default (matches `xz` default). |
| L-A05 | Encode parameters (lc, lp, pb, dict_size) are configurable via `LzmaOptions`. |
