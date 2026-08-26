# Agent restart briefs — omnizip-rs remaining work

Repo: /Users/mulgogi/src/omnizip/omnizip-rs (pure-Rust ports; `#![forbid(unsafe_code)]`
workspace-wide; NO runtime binaries — external tools are TEST ORACLES ONLY, and
NEVER unrar-as-dependency: every algorithm implemented in-house).

READ FIRST: CLAUDE.md, then ~/.claude/projects/-Users-mulgogi-src-omnizip-omnizip-rs/memory/
(esp. containers-progress.md, never-use-unrar.md, brotli/lzma/zstd-parity-progress.md).

Branch state (all pushed):
- feat/rar5-lz        — COMPLETE RAR5 (LZ 1-5+solid, BLAKE2sp, multi-volume,
                        AES-256, encrypted headers; arm fixture; 11 commits).
                        MERGE THIS FIRST — everything else branches off it.
- feat/zip64-acceptance — COMPLETE (ZIP64 acceptance + doc sweep, 4518475).
- feat/iso-rr-joliet  — WIP f92a03b (details in task 3 below).

Git rules (ABSOLUTE): never `git add -A`/`.`/`-u` — stage explicit paths, verify
with `git diff --cached --name-only`; never commit/push to main; never push tags;
rebase-merge PRs; NO AI attribution (no Co-authored-by/Generated-with trailers);
use `gh pr create --body-file` for PR bodies.

Build/test: `cargo test -p <crate>`, `cargo clippy -p <crate> --all-targets` (0 new
warnings), `cargo fmt --all`. CI = 7-group × 3-OS matrix; new crates join a group
in .github/workflows/ci.yml.

Oracles installed: 7zz, bsdtar (libarchive 3.5.3), unzip, /tmp/unrar (built
reference, instrument-able), /tmp/la_x (libarchive C harness), brew brotli.
Fixtures: ../omnizip/spec/fixtures/ (sibling Ruby checkout; rar corpus at
.../rar/libarchive_reference/). /tmp/unrar persists; rebuild with
`cd /tmp/unrar && make`; instrument with getenv guards; NOTE unpack50.cpp is
#included by unpack.cpp so `rm unpack.o` to force rebuild.

---

## TASK 1 — RAR4 LZSS + PPMd decoders + multi-part + RAR3 AES  (BIG)

Branch: feat/rar4-lz off feat/rar5-lz. Crate: omnizip-rar (src/rar3.rs =
STORE-only reader; rar5.rs shows every pattern to copy: arena stitching, split
flags, per-entry crypto, corpus tests).

1. LZSS 2.9/3.x: port /tmp/unrar/unpack15.cpp, unpack20.cpp, unpack30.cpp.
   Method byte: 0x31..0x35 → rar "version" (min(c,29)... see ReadBlockHeader in
   those files). Window sizes: 29=64K.. up to 4MB (rar3 dictionary opcodes).
2. PPMd (methods with unpack_ver 15-2x…): /tmp/unrar/model.cpp, suballoc.cpp,
   coder.cpp = PPMd var H/I + range coder; self-contained, port directly.
   VM filters (rarvm.cpp) needed for -mc archives; port if corpus demands.
3. Multi-part: same SPLIT flag stitching as rar5.rs (part1.rar/part2.rar or
   .rar/.r00 naming).
4. RAR3 encryption: crypt.cpp SetKey30 (8-byte salt, SHA1 key schedule 3×,
   AES-128-CBC with per-16-block init, tail-unpad to 16). Password 'password'
   on libarchive fixtures.
Oracle: /tmp/unrar/unrar x (compare bytes; corrupt fixtures like
ppmd_use_after_free expect clean errors). Extend omnizip-rar/tests/corpus.rs
(Rar4Reader path); raise the rejected/structured bars as files start decoding.

## TASK 2 — 7z phase C: solid write + multi-volume + encrypted headers

Branch: feat/sevenzip-phase-c off feat/rar5-lz. Crate: omnizip-sevenzip
(reader + non-solid writer exist; writer.rs SevenZipMethod::Deflate works).
1. Solid writing: multiple files → one folder (concatenate unpacked streams,
   single coder, empty-stream/file bit vectors — see reader.rs parsing to mirror).
2. Multi-volume: .7z.001/.002 split; 7zz x must reassemble.
3. Encrypted headers: 7zAES (AES-256, SHA-512 KDF — check omnizip-crypto for
   sha512; add if missing), encoded header streams.
4. LZMA2 folder coder if omnizip-lzma lzma2_compress shipped by then.
Oracle: 7zz t/x byte-exact + determinism suite. Wire solid into ozip CLI
(ozip/src/container.rs OutputFormat::SevenZip).

## TASK 3 — ISO writer: Rock Ridge + Joliet emission  (WIP on branch!)

Branch: feat/iso-rr-joliet (f92a03b; builds, existing tests green). The final
extent layout is IN PLACE: [16]PVD [17]SVD-slot [18]term [19..22] path-table
slots [23..] PVD dirs / Joliet placeholders / files. Helpers susp/susp_nm/
susp_px/susp_sl/susp_sp/ucs2 exist (writer.rs bottom). Node carries full_name,
mode, is_link, link_target, j_extent, j_dir_size.

NEXT CONCRETE STEPS:
1. record_bytes(): add `su: &[u8]` param — bytes after name-pad, then final
   pad so TOTAL record length stays even. PVD root record gets susp_sp().
2. directory_bytes(): child records get rr_area(&cn.full_name, cn.mode,
   link_target if cn.is_link). Root "." record gets susp_sp().
3. Emit real SVD replacing the sector-17 boot placeholder: type 2, CD001,
   version 1, escape bytes 88..91 = 25 2F 45 ("%/E"), UCS-2BE volume ids
   (8..40/40..72), both-endian volume_space/blocksize like PVD, joliet path
   table size/locations (21 L, 22 M), root record at 156..190 with UCS-2
   "\0" name (34 bytes total, name_len=1).
4. Joliet path tables: like build_path_table but names = ucs2(full_name)
   (dirs: full_name WITHOUT trailing slash — reader trims anyway); name_len
   counts BYTES. Fill sectors 21/22.
5. Joliet directory bytes: replace zero placeholders; records via
   record_bytes(..., su=&[]) with name = ucs2(full_name) (files NO ";1"),
   extents = j_extent/j_dir_size; children in the same sorted order.
6. Reader: add rock_ridge_symlink() to DirectoryRecord (parse SL: data[0]=
   flags, then component bytes) + EntryKind::Symlink in entries() when SL
   present (reader.rs entries() + lib.rs, mirroring rock_ridge_mode()).
7. Tests: round-trip long mixed-case names (>8.3, >31), subdirs, symlink
   with target, mode bits; cross-check xorriso/isoinfo if installed.
Volume space must count ALL sectors incl. SVD + joliet extents (already does —
verify volume_space == out.len()/2048).

## TASK 4 — DONE (feat/zip64-acceptance, 4518475): merge as-is.

## TASK 5 — btopt-style optimal parsing (brotli q10/11 tier)

Branch: feat/btopt off feat/rar5-lz. Crate: omnizip-brotli.
Net-new (absent even from the Ruby reference). Generalize the LZMA optimum
parser pattern (omnizip-lzma/src/encoder/optimum.rs) into a cost-model DP:
backward pass over positions, candidates {literal, all matches incl.
static-dictionary refs + max-distance, rep-codes}, scored by context-modeled
bit costs from the existing brotli cost model; forward emission of the
optimal chain. Wire into q10/11 tier (current = zopfli-style short-match,
task #308 port). ACCEPTANCE: byte-deterministic; ratio target vs C
`brotli -q 11` (brew oracle); full tests green; omnizip-bench row.

---

Suggested agent split (independent, no file overlap): A=T1(rar4), B=T2(7z),
C=T3(iso), D=T5(btopt). Merge order: rar5-lz → zip64 → iso → rar4 → 7z → btopt.
After all merge: bump 0.21.0 (same 9-Cargo.toml pattern), release.
