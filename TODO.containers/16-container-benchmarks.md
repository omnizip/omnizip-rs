# 16 — Archive-level benchmarks

- **Priority:** P1
- **Depends on:** [02](02-tar.md)+[04](04-zip.md) minimum; more formats as they land
- **Estimated effort:** 1 week
- **Crate:** `omnizip-bench` (new archive mode)

## Goal

Extend `omnizip-bench` with container cases: create/extract a fixed tree
(e.g. the Silesia files as a directory) per format, reporting size, encode
and decode throughput, and determinism — alongside the codec cases. Adds the
"vs system tools" comparisons the website publishes (tar+gzip vs `tar czf`,
zip vs `zip -9`, xz vs `xz -6`) so the published tables regenerate from one
command.

## Acceptance

- [ ] `omnizip-bench --archives --corpus silesia-tree` produces the four
      columns the website table needs
- [ ] Determinism double-run assert extended to archives
- [ ] CI regression budget entries (like the codec baseline.json) for
      archive create/extract time and size
