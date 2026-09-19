#!/usr/bin/env bash
# Generate RAR5 archives with recovery records over deterministic
# content. Usage: gen_rar_rr.sh <output-dir>
set -euo pipefail
OUT="${1:?output dir}"
DEST="$OUT/rr"
mkdir -p "$DEST"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# Deterministic source tree: text + binary, sizes spanning several
# RS sectors.
mkdir -p "$work/src"
printf 'recovery record fixture: the quick brown fox\n%.0s' {1..200} > "$work/src/text.txt"
python3 - "$work/src/binary.bin" <<'PY'
import sys
with open(sys.argv[1], "wb") as f:
    for i in range(60000):
        f.write(bytes([(i * 31 + i // 251) & 0xFF]))
PY
head -c 50000 /dev/zero > "$work/src/zeros.bin"

# RAR5 format is rar >= 5.0's default; -rr<N> adds a recovery
# record of N percent.
rar a -idq -ma5 -m1 -rr3 "$DEST/small_rr3.rar" "$work/src/text.txt" >/dev/null
rar a -idq -ma5 -m5 -rr10 "$DEST/mixed_rr10.rar" "$work/src/text.txt" "$work/src/binary.bin" "$work/src/zeros.bin" >/dev/null

# A pristine twin WITHOUT the record, same content — the repair
# oracle pair (corrupt twin + RR archive -> repaired == pristine).
rar a -idq -ma5 -m1 "$DEST/small_plain.rar" "$work/src/text.txt" >/dev/null
rar a -idq -ma5 -m5 "$DEST/mixed_plain.rar" "$work/src/text.txt" "$work/src/binary.bin" "$work/src/zeros.bin" >/dev/null

echo "generated:"
ls -la "$DEST"
