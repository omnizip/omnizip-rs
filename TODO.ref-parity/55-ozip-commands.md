# Task 55 — ozip commands: verify / repair / parity / metadata / profile

Status: partially done (2026-09-19 — `verify` and `metadata`
shipped with pin tests; verify = full structural + checksum read
of every entry, zip CRC32 verified by the reader on read; red on
corruption with per-entry FAIL lines. REMAINING in this task:
`parity create/verify/repair` (wraps omnizip-par2), `archive
repair` (port Ruby's semantics), `profile list|show` (needs 60).
clap migration deferred: the hand-rolled router grew the commands
without a new dependency — revisit if the command count doubles)

## Gap

Ruby `commands/`: archive_create, archive_extract, archive_list,
archive_verify, archive_repair, compress, decompress, list,
metadata, parity_create, parity_verify, parity_repair,
profile_list, profile_show — 14 commands. ozip today: `c/x/t/l` +
`--formats` hand-rolled in `ozip/src/main.rs` (two source files
total).

## Scope

- (deferred) Move ozip to clap subcommands (clap is already a workspace dep via
  bench) keeping the short forms `c/x/t/l` as aliases — no behavior
  change to existing commands.
- `ozip verify <archive>`: structural check + per-entry checksum
  recompute via task 52's registry (crc32 for zip/store, per-format
  digests); report JSON + human.
- `ozip repair <archive>`: port Ruby `archive_repair_command.rb`
  semantics — PAR2-based recovery when a parity set exists; local
  header salvage for truncated zips (scope exactly what Ruby does,
  no more).
- `ozip parity create|verify|repair <path>`: CLI over the shipped
  omnizip-par2 crate (verify/repair already exist as library API).
- `ozip metadata <path>`: entry table (name, size, method, mtime,
  checksum, attrs) as JSON/YAML — port `metadata_command.rb` fields.
- `ozip profile list|show` (stub until 60, then real).

## Acceptance

- Each command has fixtures: a good archive, a corrupted one (bit
  flip), a truncated one; verify distinguishes checksum vs
  structural failure.
- parity round-trip: create → corrupt → repair → byte-exact.
- Determinism: `verify`/`metadata` output is stable (sorted keys) —
  these feed downstream tooling.

## References

`../omnizip/lib/omnizip/commands/*.rb` (semantics per command),
`archive_handlers/` for per-format verify hooks.
