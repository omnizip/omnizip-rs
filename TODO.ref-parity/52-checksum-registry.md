# Task 52 — checksum registry (pluggable checksum family)

Status: open (part of task 51 phase 1)

## Gap

Ruby: `checksum_registry.rb` + `checksums/{crc32,crc64,crc_base,
verifier}.rb` — a registry (name → class, duplicate-registration
rejection, `available`) and a `verifier` that checks stored vs
computed digests. Rust today: checksums are per-crate ad hoc
(crc32 in codecs/zip, blake2sp + RustCrypto digests in
omnizip-crypto, xxh64 inside zstd). Nothing is selectable by name.

## Scope

New crate `omnizip-checksum`:

- `trait Checksum { fn update(&mut self, data: &[u8]); fn finish(&self) -> Digest; }`
  with `Digest` as an owned byte/`u64` enum.
- Registry in the house style of `CodecRegistry`: name-keyed,
  duplicate-name rejection, `available()`.
- Families: **crc32** (wrap crc32fast — reuse, do not reimplement),
  **crc64-ecma-182 / crc64-xz** (the gem's crc64; implement the two
  polys in-house — they are ~30 lines each with tables), plus a
  pass-through re-export of the crypto crate's sha2/blake2sp where a
  `Checksum` impl makes sense.
- `Verifier`: given stored digest + algorithm name, stream-verify.

## Acceptance

- CRC vectors: crc32 (zlib/png vectors), crc64-xz (xz --check=crc64
  on fixture files), crc64-ecma (RFC 4057-era vectors).
- Registry: duplicate registration errors; unknown name lists
  available.
- Differential: crc32/crc64 values match `xz -l` checksums on the
  shared fixture corpus.
- Feeds task 55 (ozip verify).

## References

`../omnizip/lib/omnizip/checksum_registry.rb`,
`../omnizip/lib/omnizip/checksums/` (semantics reference).
