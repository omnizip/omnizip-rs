# 04 — ZIP (+ ZIP64)

- **Priority:** P0
- **Depends on:** [01](01-archive-core.md), `omnizip-deflate`, `omnizip-deflate64`
- **Estimated effort:** 2–3 weeks
- **Crate:** `omnizip-zip`

## Goal

Full ZIP read/write: local + central directory, ZIP64 (sizes, offsets,
entries > 4 GiB / > 65,535), methods 0 (store), 8 (deflate), 9 (deflate64),
data descriptors, UTF-8 names. Encryption is a separate task
([05](05-zip-encryption.md)).

## Ruby → Rust module map

| Ruby source | Rust module | LOC |
|---|---|---:|
| `formats/zip/reader.rb` | `reader.rs` | ~700 |
| `formats/zip/writer.rb` | `writer.rs` | ~640 |
| `formats/zip/*.rb` (entry, central dir, extra fields) | `fields.rs`, `extra.rs` | ~320 |

## Acceptance

- [ ] Round-trips the Ruby ZIP fixture corpus byte-correctly
- [ ] `unzip -t` clean + `unzip` byte-exact on outputs; `zip -9` inputs decode
- [ ] ZIP64 exercised at >4 GiB and >65,535 entries
- [ ] Deterministic archives per [17](17-determinism-normalization.md)
