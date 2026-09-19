# Task 60 — compression profiles + content-class detector

Status: open (part of task 51 phase 3)

## Gap

Ruby `profile/`: named profiles mapping content class → codec +
level + options, plus `profile_detector` (MIME sniff via
`Omnizip::FileType` → `suitable_for?` per profile → priority
selection). Known mappings: fast = Deflate L1; maximum = LZMA2 L9
solid; binary = level 6 (check codec in `binary_profile.rb`);
archive = level 0 (store-ish, solid — see `archive_profile.rb`);
plus balanced and custom. Rust: nothing.

## Scope

New crate `omnizip-profile` (depends on omnizip-codecs only):

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
