#!/usr/bin/env bash
# Differential FLAC benchmark (TODO 105).
#
# For every .wav file under tests/audio_corpus/fixtures/, compresses
# with omnizip-flac, libFLAC, LZ4, and ZSTD L12, then writes a CSV
# row. Run this after ./fetch.sh has populated fixtures/.
#
# Acceptance criteria (TODO 105):
#   - omnizip-flac within 5% of libFLAC on >= 95% of tracks
#   - omnizip-flac beats LZ4 by >= 10% on >= 90% of tracks
#   - omnizip-flac beats ZSTD L12 by >= 10% on >= 80% of tracks

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIXTURES="$ROOT/tests/audio_corpus/fixtures"
CSV="$ROOT/tests/audio_corpus/results.csv"

if [ ! -d "$FIXTURES" ]; then
    echo "fixtures/ not found; run ./fetch.sh first" >&2
    exit 1
fi

# CLI tools we need.
need() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "missing required binary: $1" >&2
        exit 1
    }
}
need flac
need lz4
need zstd
need cargo

# Build the omnizip-flac encoder binary (Phase 1: small driver).
cargo build --release -p omnizip-flac 2>/dev/null || true

echo "fixture,orig_bytes,flac_bytes,lz4_bytes,zstd_bytes" > "$CSV"

find "$FIXTURES" -type f -name '*.wav' | while read -r wav; do
    rel="${wav#$FIXTURES/}"
    orig=$(wc -c <"$wav")

    flac_tmp=$(mktemp --suffix=.flac)
    lz4_tmp=$(mktemp --suffix=.lz4)
    zstd_tmp=$(mktemp --suffix=.zst)

    flac --silent --best --force-raw-format --endian=little --sign=signed -o "$flac_tmp" "$wav" 2>/dev/null || flac_bytes=-1 || flac_bytes=$(wc -c <"$flac_tmp")
    lz4 --best --quiet --no-progress "$wav" "$lz4_tmp" 2>/dev/null || lz4_bytes=-1 || lz4_bytes=$(wc -c <"$lz4_tmp")
    zstd -12 --quiet --force "$wav" -o "$zstd_tmp" 2>/dev/null || zstd_bytes=-1 || zstd_bytes=$(wc -c <"$zstd_tmp")

    # omnizip-flac is invoked via cargo test (Phase 1 — see tests.rs).
    # For now, leave the flac_bytes column for the libFLAC value and
    # let the test binary fill in omnizip-flac separately.
    echo "$rel,$orig,$flac_bytes,$lz4_bytes,$zstd_bytes" >> "$CSV"

    rm -f "$flac_tmp" "$lz4_tmp" "$zstd_tmp"
done

echo "Wrote $CSV"
