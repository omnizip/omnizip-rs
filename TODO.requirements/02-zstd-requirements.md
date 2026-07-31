# 02 — Zstandard requirements

## Functional

| ID | Requirement |
|---|---|
| Z-F01 | Decode every `.zst` file produced by reference `zstd` at levels 1–22. |
| Z-F02 | Encode at levels 1–22; output round-trips through reference `zstd -d`. |
| Z-F03 | Encode output is byte-identical to omnizip Ruby at matching level. |
| Z-F04 | Support dictionary mode (encode + decode with a trained dictionary). |
| Z-F05 | Support multi-frame files (concatenated frames). |
| Z-F06 | Support content checksum verification. |

## Non-functional

| ID | Requirement | Target |
|---|---|---|
| Z-N01 | Decode throughput | ≥ 500 MB/s single-core on Apple M1 |
| Z-N02 | Encode throughput at level 1 | ≥ 100 MB/s single-core |
| Z-N03 | Encode throughput at level 6 | ≥ 50 MB/s single-core |
| Z-N04 | Ratio vs reference `zstd -6` on Silesia | within 5% |
| Z-N05 | Ratio vs reference `zstd -19` on Silesia | within 3% |

## Error handling

| ID | Requirement |
|---|---|
| Z-E01 | Reject invalid magic number `0xFD2FB528` with `OmnizipError::Corrupt`. |
| Z-E02 | Reject reserved bit set in frame header descriptor. |
| Z-E03 | Reject `windowLog > 31` (window > 2 GiB) unless `--ultra` enabled. |
| Z-E04 | Reject block size > `3 * windowSize / 2`. |
| Z-E05 | Detect content checksum mismatch. |
| Z-E06 | Return `OmnizipError::LengthMismatch` if output ≠ `expected_len`. |

## API

| ID | Requirement |
|---|---|
| Z-A01 | `compress(plaintext, level)` returns `Result<Vec<u8>, ZstdError>`. |
| Z-A02 | `decompress(compressed, expected_len)` returns `Result<Vec<u8>, ZstdError>`. |
| Z-A03 | `ZstdLevel` is an enum: `Fastest`(1), `Fast`(3), `Default`(6), `Better`(12), `Best`(22). |
| Z-A04 | Level 6 (`Default`) is the default. |
| Z-A05 | `ZstdOptions` includes `window_log`, `chain_log`, `hash_log`, `search_log`, `min_match`, `target_length`, `strategy`. |
