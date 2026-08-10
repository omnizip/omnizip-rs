#!/usr/bin/env bash
# Profile-Guided Optimization (PGO) build script (TODO 270).
#
# Workflow:
#   1. Build instrumented release binaries.
#   2. Run representative workloads to collect profile data.
#   3. Merge profile data with llvm-profdata.
#   4. Rebuild with the profile fed back to the compiler.
#
# Typical gain: 5-15% throughput on hot paths.
#
# Usage:
#   ./scripts/build-pgo.sh                # full PGO build
#   ./scripts/build-pgo.sh --instrument   # only step 1
#   ./scripts/build-pgo.sh --collect      # only step 2 (requires instrumented build)
#   ./scripts/build-pgo.sh --merge        # only step 3
#   ./scripts/build-pgo.sh --optimize     # only step 4

set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PGO_DATA_DIR="${PGO_DATA_DIR:-${ROOT}/target/pgo-data}"
PGO_PROFILE="${PGO_PROFILE:-${ROOT}/target/pgo.profdata}"
PGO_BENCH_BIN="${ROOT}/target/release/omnizip-bench"

step() {
    printf '\n\033[1;33m[pgo] %s\033[0m\n' "$*"
}

instrument() {
    step "Instrumented build"
    RUSTFLAGS="-Cprofile-generate=${PGO_DATA_DIR}" \
        cargo build --release -p omnizip-bench
    mkdir -p "${PGO_DATA_DIR}"
}

collect() {
    step "Collecting profile data via representative workloads"
    # Run brotli/zstd/lzma/lz4 benchmarks across synthetic inputs.
    # Extend with real corpora when available (TODO 247).
    "${PGO_BENCH_BIN}" --synthetic 65536 --iterations 2 || true
}

merge_profile() {
    step "Merging profile data"
    local profdata_cmd
    profdata_cmd="$(find "$(rustc --print sysroot)" -name llvm-profdata -type f | head -1)"
    if [[ -z "${profdata_cmd}" ]]; then
        echo "[pgo] llvm-profdata not found in rust toolchain" >&2
        exit 1
    fi
    "${profdata_cmd}" merge \
        -o "${PGO_PROFILE}" \
        "${PGO_DATA_DIR}"/*.profraw
    echo "[pgo] Profile written to ${PGO_PROFILE}"
}

optimize() {
    step "Optimized build using profile"
    RUSTFLAGS="-Cprofile-use=${PGO_PROFILE}" \
        cargo build --release --workspace
    echo "[pgo] Optimized binaries in target/release/"
}

case "${1:-all}" in
    --instrument) instrument ;;
    --collect) collect ;;
    --merge) merge_profile ;;
    --optimize) optimize ;;
    all)
        instrument
        collect
        merge_profile
        optimize
        ;;
    *)
        echo "Usage: $0 [--instrument|--collect|--merge|--optimize]"
        exit 1
        ;;
esac
