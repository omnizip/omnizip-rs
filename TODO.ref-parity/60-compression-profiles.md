# Task 60 — compression profiles + content-class detector

Status: done (2026-09-19 — omnizip-codecs::profile extended (SSOT
home; the intent-based Profile stays the codec-facing API): the 5
named presets ported FIELD-BY-FIELD (fast=deflate/1, balanced=
deflate/6, binary=lzma2/6+bcj_x86, archive=store/0, maximum=lzma2/9
+auto+solid; lzma2→LZMA codec id, store→STORE), CustomProfile
inheritance semantics, detect_profile(): magic sniff (ELF/Mach-O/
PE→binary; zip/gz/xz/zst/bzip2/png/jpg/gif/mp3→archive) then
content_type → Ruby priority selection incl. the VESTIGIAL :text
priority entry (no text profile ships in Ruby's registry either —
balanced wins; quirk documented). Module went pub. Mapping-table
+ detector tests pin both directions

## Gap

Ruby `profile/`: named profiles mapping content class → codec +
level + options, plus `profile_detector` (MIME sniff via
`Omnizip::FileType` → `suitable_for?` per profile → priority
selection). Known mappings: fast = Deflate L1; maximum = LZMA2 L9
solid; binary = level 6 (check codec in `binary_profile.rb`);
archive = level 0 (store-ish, solid — see `archive_profile.rb`);
plus balanced and custom. Rust: nothing.

## Scope

(SSOT decision: no new crate — profile.rs in omnizip-codecs is the single home. Originally sketched as a new crate; the existing intent-based Profile + private content_type made extension the SSOT-correct call.)

- `Profile` struct: name, codec, level, solid flag, options;
  `registry()` with the six shipped profiles ported FIELD BY FIELD
  from the Ruby classes (they are the reference — do not invent
  mappings; where the Ruby profile targets a codec by symbol,
  translate to our CodecId and record the mapping in the task file).
- `ProfileDetector`: content sniffing via archive-core's
  format/content detection where exposed, else in-crate
  classification reusing the bench's content classes (text /
  binary-executable / multimedia / already-compressed / repetitive
  — `omnizip-bench/src/synthetic.rs` vocabulary). Selection =
  suitable_for? set ∩ priority order, deterministic tie-break.
- `custom` profile = user-supplied overrides (serde-light or manual
  builder; no serde dependency unless already in tree).
- ozip: `--profile fast|balanced|maximum|binary|archive|custom`
  on create; `profile list/show` (task 55's stub goes live).

## Acceptance

- Mapping table test: every shipped profile resolves to the exact
  codec+level+solid triple the Ruby class declares (documented in
  this file when ported).
- Detector: shared fixture corpus (text, ELF, PNG, MP3, zip-in-zip,
  zeros) → same profile choice as Ruby's ProfileDetector on the
  same files (differential; where Ruby uses MIME db unavailable to
  us, nearest-class mapping documented per fixture).
- Determinism: detection is a pure function of the first N bytes +
  size (declare N; no fs-stat beyond size/name suffix).

## References

`../omnizip/lib/omnizip/profile/*.rb` (all eight files).
