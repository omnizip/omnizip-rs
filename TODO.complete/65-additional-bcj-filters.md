# 65 — Additional BCJ filters

## Gap

`omnizip-filters` implements only BCJ-x86 (filter ID 0x04 in the XZ
filter chain). The XZ spec defines 7 BCJ filters:

| ID    | Architecture | Status |
|-------|-------------|--------|
| 0x04  | x86         | ✅ done |
| 0x05  | PowerPC     | ❌ missing |
| 0x06  | IA-64       | ❌ missing |
| 0x07  | ARM         | ❌ missing |
| 0x08  | ARM-Thumb   | ❌ missing |
| 0x0A  | SPARC       | ❌ missing |
| 0x0B  | ARM-64      | ❌ missing |

Each BCJ filter rewrites branch/call instructions so their target
addresses become relative (and thus redundant under compression).
The decoder reverses the transform.

## Algorithm per filter

All BCJ filters share the same skeleton:
1. Scan the input in 4-byte (or 2-byte for ARM-Thumb) steps.
2. Match the architecture's branch/call opcode pattern.
3. Rewrite the target field from absolute to relative.

Differences are in the opcode tables and the address arithmetic.

## Files

- `omnizip-filters/src/bcj/mod.rs` — trait + dispatcher.
- `omnizip-filters/src/bcj/x86.rs` — existing.
- `omnizip-filters/src/bcj/{ppc,ia64,arm,armthumb,sparc,arm64}.rs` — new.
- Port from `~/src/external/xz-utils/src/liblzma/simple/`.

## Test strategy

- For each filter: synthetic executable with known branch pattern.
- Round-trip: apply forward then reverse, assert identity.
- Ratio: the filtered version should compress 5-10% better with LZMA.
