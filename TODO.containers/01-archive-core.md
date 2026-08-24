# 01 — Archive core: Entry, IO, ArchiveHandler

- **Priority:** P0 (every format task depends on this)
- **Depends on:** —
- **Estimated effort:** 1–2 weeks
- **Crate:** `omnizip-archive-core` (new)

## Goal

The shared container layer: the `Entry` data model, IO source/sink
abstraction, and the `ArchiveReader` / `ArchiveWriter` traits that every
format crate implements — the container analogue of `omnizip-codecs`.
Includes the security boundary (path traversal / symlink / bomb guards) so
no format re-implements it.

## Ruby → Rust module map

| Ruby source | Rust module | LOC | Notes |
|---|---|---:|---|
| `omnizip/entry.rb` | `entry.rs` | 44 | mix-in contract |
| `omnizip/archive_handler.rb` + `archive_handlers.rb` | `handler.rs` | ~150 | registry consolidation (FormatRegistry was deleted in Ruby; keep the consolidated shape) |
| `omnizip/io.rb` + `io/*.rb` | `io.rs` (`Source`, `Sink` enums) | ~400 | File/BufReader/Memory sources, path/file/stream sinks |
| `omnizip/file_type.rb` | `detect.rs` | ~120 | magic-byte sniffing — feeds Excavate later |
| `omnizip/extraction/` | `extraction.rs` | ~300 | selective extraction: glob/regex/predicate |
| `omnizip/error.rb` | `error.rs` | ~80 | unified container error |
| (new, security) | `security.rs` | ~250 | see [21](21-security-hardening.md) |

## Design notes

- `Entry` carries name, size, mtime, mode, link target, checksum, method —
  one struct for all formats; per-format fields live in the format crate.
- Trait shape mirrors the codecs: `ArchiveReader { entries, read(entry), extract_all }`,
  `ArchiveWriter { add_file, add_dir, finish }`. Dispatch through a
  `FormatRegistry` keyed by format id.
- Iteration is `Iterator<Item = Entry>` — the Ruby Enumerable contract.

## Acceptance

- [ ] `omnizip-archive-core` with traits + registry, `#![forbid(unsafe_code)]`
- [ ] Security guards unit-tested (zip-slip, `../`, absolute paths, symlink
      escape, ratio/size-bomb limits) and ON by default with opt-outs
- [ ] Format sniffing passes the Ruby `file_type` spec vectors
- [ ] 02-tar implements the traits end-to-end against this crate
