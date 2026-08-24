# 08 — RAR4 read

- **Priority:** P2
- **Depends on:** [07](07-rar5.md) (shared `rar/common`)
- **Estimated effort:** 1–2 weeks
- **Crate:** `omnizip-rar` (`rar3` module)

## Goal

RAR3/RAR4 read-only: all compression methods (LZSS 2.9/3.x variants, PPMd,
audio filters), archive headers, multi-part sets, encryption. No writer —
reference `rar` is shareware and the Ruby side is read-mostly too
(write = STORE/FASTEST/NORMAL only; port that subset only if a consumer
asks).

## Ruby → Rust module map

| Ruby source | Rust module | LOC |
|---|---|---:|
| `formats/rar3/reader.rb` + method decoders | `rar3/reader.rs` | 746 |

## Acceptance

- [ ] Decodes the libarchive RAR4 compatibility corpus (103 files) byte-exactly
- [ ] Multi-part + encrypted RAR4 archives extract correctly
