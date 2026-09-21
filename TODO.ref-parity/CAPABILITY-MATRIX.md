# Ruby ⇄ Rust Capability Matrix

The complete surface: every codec, format, subsystem, and command the
Ruby gem (`omnizip`) exposes, its Rust counterpart, and the
acceleration status of the Ruby→Rust tier. Legend:

- **Ruby**: pure-Ruby implementation (`lib/omnizip/…`)
- **Rust**: crate in this workspace
- **ozip**: the Rust CLI covers the Ruby command
- **Tier**: how the Ruby gem accelerates through the Rust cdylib —
  `auto` = decode routes to Rust whenever the library loads,
  `fallback` = a Rust error falls back to the Ruby core (auto mode is
  never worse than pure Ruby), `—` = not wired (see Notes)

## 1. Codecs (algorithm level)

| Codec | Ruby (`algorithms/`) | Rust crate | FFI name | Tier (decode) | Notes |
|---|---|---|---|---|---|
| bzip2 | bzip2.rb | omnizip-bzip2 | `bzip2` | auto | wired since task 62 |
| zstd | zstandard.rb | omnizip-zstd | `zstd` | auto | wired 2026-09-20; dict frames stay pure-Ruby (params beyond the 3-arg FFI contract) |
| lzma-alone | lzma.rb | omnizip-lzma | `lzma-alone` | auto | LZMA1 alone (13-byte header) |
| xz (container) | xz_impl | omnizip-lzma | `xz` | auto | full container incl. checks |
| lzip | formats/lzip.rb | omnizip-lzma | `lzip` | auto | decode-only (no lzip writer on either side) |
| deflate (zlib-framed) | deflate.rb | omnizip-libdeflate | `zlib` | auto | the Ruby encoder emits zlib-framed streams (`78 9C`) |
| deflate (raw RFC 1951) | — (zip-internal) | omnizip-libdeflate | `deflate` | — | `compress_raw`/`decompress_raw_unknown_len`; zip/tar.gz paths use it in Rust |
| deflate64 | deflate64.rb (zlib-framed) | omnizip-deflate64 | `deflate64` | — | real wire deflate64 in Rust (zip method 9); the Ruby algorithm class is zlib-framed → rides the `zlib` name |
| gzip (container) | formats/gzip.rb | archive-core formats::gzip | `gzip` | auto | Ruby writer FIXED (was double-headered, nonstandard); reader keeps decoding legacy files |
| ppmd7 | ppmd7.rb | omnizip-ppmd | `ppmd7:o{order}:m{mem}` | auto (both) | tier-implemented 2026-09-21; the pure-Ruby core cannot decode its own output (root-only decoder, 100-symbol cap) — fallback only |
| ppmd8 | ppmd8.rb | omnizip-ppmd | `ppmd8:o{order}:m{mem}` | auto (both) | tier-implemented 2026-09-21; the pure-Ruby core raises NotImplementedError — fallback only |
| brotli | — | omnizip-brotli | — | — | Rust-only (in-house from RFC); the gem has no brotli |
| lz4 / snappy / fsst / glza / flac / blosc / zpaq / ricepp | — | respective crates | — | — | Rust-only codecs |
| filters: BCJ x86/ARM/ARMThumb/ARM64/IA64/PPC/SPARC, BCJ2, delta, shuffle | filters/ | omnizip-filters | — | — | Rust side used by the xz container (all IDs incl. ARM64 start-offset); the gem's filter pipeline stays Ruby |

Encode stays pure-Ruby in `auto` mode by policy
(`RUST_ENCODE_IDENTICAL` empty): frame-byte stability for content
addressing; every FFI name above does expose compress for forced
`OMNIZIP_BACKEND=rust` mode.

## 2. Archive formats

| Format | Ruby (`formats/`) | Rust crate | Read | Write | Notes |
|---|---|---|---|---|---|
| zip | zip/ | omnizip-zip | ✓ | ✓ | zip64, WinZip-AES, legacy ZipCrypto read (2026-09-20); AES write |
| tar (+gz/bz2/xz/zst) | tar.rb | omnizip-tar + wrappers | ✓ | ✓ | |
| cpio | cpio/ | omnizip-cpio | ✓ | ✓ | |
| 7z | seven_zip/ | omnizip-sevenzip | ✓ | ✓ | incl. header encryption, split archives, BCJ2 |
| rar3/4 | rar3.rb, rar/ | omnizip-rar | ✓ | ✓(store) | LZ/PPMd/AES; Ruby rar3 writer is store-class too |
| rar5 | rar5.rb, rar/ | omnizip-rar | ✓ | ✓(store) | volumes, AES, RR parse (percent+CRC); **RS repair impossible for anyone** (unrar 7.2.7 has none; the Ruby recover_with_reed_solomon was a nil placeholder) |
| iso | iso/ | omnizip-iso | ✓ | ✓ | joliet + rock-ridge |
| rpm | rpm/ | omnizip-rpm | ✓ | ✓ | |
| xar | xar/ | omnizip-xar | ✓ | ✓ | |
| ole (compound) | ole/ | omnizip-ole | ✓ | — | read-only on both sides |
| msi (+cab) | msi/ | omnizip-ole/src/msi.rs | ✓ | — | |
| gzip/bzip2/xz/zstd/lzip/lzma-alone files | formats/*.rb | archive-core + codecs | ✓ | ✓ (no lzip write) | FNAME now authoritative on read |

## 3. Commands (Ruby `commands/` ⇄ `ozip`)

| Ruby command | ozip | Status |
|---|---|---|
| compress_command | `ozip c` / `ozip c -f <codec>` | ✓ |
| decompress_command | `ozip -d` | ✓ |
| archive_create/extract/list | `ozip c/x/t/l` | ✓ (+`--threads`) |
| metadata_command | `ozip metadata` | ✓ |
| archive_verify_command | `ozip verify` | ✓ |
| archive_repair_command | `ozip repair` | ✓ (loud-fail + par2 guidance; in-archive RAR RS repair impossible for anyone) |
| parity_create/verify/repair | `ozip parity create/verify/repair` | ✓ (real RS via omnizip-par2) |
| profile_list/show | `ozip profile list/show` | ✓ |

## 4. Cross-cutting subsystems

| Ruby subsystem | Rust counterpart | Status |
|---|---|---|
| chunked (memory manager/reader/writer) | omnizip-codecs::chunked | ✓ (task 57) |
| parallel engine | parallel_batch + extract/create engines | ✓ (task 58, thread-invariant output) |
| progress/ETA | codecs::progress (EMA tracker) | ✓ core; Ruby's 4 estimator variants stay Ruby-side |
| profile/detector | codecs::profile | ✓ (task 60) |
| converter (+7z⇄zip strategies) | ozip convert (extract-repack) | ✓; dedicated entry-at-a-time strategies follow-up |
| password providers | archive-core::password | ✓ (+ZipCrypto read) |
| checksums crc32/crc64/verifier | omnizip-checksum | ✓ (task 52) |
| buffer/memory archive | writers' in-memory finish_bytes | ✓ |
| extraction selective patterns (glob/regex/predicate) | ozip x --include GLOB | glob shipped 2026-09-20 (full name or path-suffix); regex/predicate stay Ruby-side (library-level API) |
| io/buffered, pipe | streaming traits | ✓ (task 57) |
| file_type/mime | codecs::content_type | ✓ |
| link_handler | write_entry_secured + SecurityPolicy | ✓ |
| rubyzip_compat | — | dropped by owner |
| FSST/GLZA/etc | Rust-only crates | n/a |

## 5. Acceleration tier wiring (2026-09-20)

`Backends.decompress` seams: bzip2 (prior), zstd, zlib×2 (deflate.rb +
deflate64.rb), gzip, xz, lzma-alone, lzip — all with cross-implementation
differential specs (`spec/omnizip/implementations/rust_tier_codecs_spec.rb`).
Decode failure on the Rust side falls back to the Ruby core (auto mode
strictly ≥ pure Ruby). `Library.forget!` fixed (was assign-nil, which
poisoned the memo).

## 6. Open items (ranked)

None. The last open item — archive-level acceleration — shipped
2026-09-21 (see §7).

Closed since the last revision: PPMd (tier-implemented with
param-carrying names — the Ruby cores were provably non-functional);
bad-1-lzma2-7 (xz-utils corpus 42/42); the StringIO#to_s corruption
trap (inspect text was being compressed) and the prepare_output
StringIO-discard bug.

## 7. Archive-level tier (2026-09-21)

`ozip_arch_open/count/entry_name/entry_size/read_entry/close` on the
cdylib: an `ArchHandle` over any multi-entry `ArchiveReader` (zip,
tar, cpio, 7z, rar3/4/5, plus iso/rpm/xar/ole via a probe chain),
with passwords via `from_bytes_with_password`. Single-file compressed
formats (gzip/bzip2/xz/zstd/lzip/lzma-alone) stay on the codec-level
tier — they have one implicit entry, which the codec name already
covers. Ruby side: `Implementations::Rust::Archive` (Fiddle bindings,
block-form `open`), `Backends.archive_entry_names` /
`archive_read_entry` (nil → Ruby fallback), wired into
`ZipHandler#list` (names path) and `#read_entry`. Generic over every
format the handle probes — other handlers can adopt the same two seam
calls.
