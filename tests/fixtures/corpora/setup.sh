#!/usr/bin/env bash
# Download real-world compression corpora on demand.
#
# Usage:
#   ./setup.sh                # download all
#   ./setup.sh silesia        # download one
#   ./setup.sh --list         # list what's available
#
# Files are placed under tests/fixtures/corpora/<name>/.

set -euo pipefail

CORPORA_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

declare -A URLs=(
    ["silesia"]="https://sun.aei.polsl.pl/~sdeor/silesia/silesia.zip"
    ["enwik8"]="https://mattmahoney.net/dc/enwik8.zip"
    ["calgary"]="https://www.data-compression.info/files/corpora/calgarycorpus.zip"
    ["canterbury"]="https://corpus.canterbury.ac.nz/resources/cantrbry.zip"
)

declare -A PostScripts=(
    ["silesia"]="unzip -o -d silesia"
    ["enwik8"]="unzip -o"
    ["calgary"]="unzip -o -d calgary"
    ["canterbury"]="unzip -o -d canterbury"
)

list_corpora() {
    echo "Available corpora:"
    for name in "${!URLs[@]}"; do
        local target_dir="${CORPORA_DIR}/${name}"
        if [[ -d "${target_dir}" ]]; then
            echo "  [installed] ${name}"
        else
            echo "  [pending]   ${name}"
        fi
    done
}

download_one() {
    local name="$1"
    local url="${URLs[${name}]:-}"
    if [[ -z "${url}" ]]; then
        echo "Unknown corpus: ${name}" >&2
        list_corpora
        exit 1
    fi

    local target_dir="${CORPORA_DIR}/${name}"
    if [[ -d "${target_dir}" && "${FORCE:-0}" != "1" ]]; then
        echo "[skip] ${name} already exists at ${target_dir}"
        return 0
    fi

    mkdir -p "${target_dir}"
    echo "[fetch] ${name} from ${url}"
    local tmp_zip="${target_dir}/${name}.zip"
    curl --fail --location --silent --show-error --output "${tmp_zip}" "${url}"

    echo "[unpack] ${name}"
    (
        cd "${target_dir}"
        # PostScripts values are "unzip <flags> [target dir]"
        # We always unzip into the corpus directory itself.
        if [[ "${name}" == "enwik8" ]]; then
            unzip -o "${name}.zip"
            # enwik8 unzips to a single file in cwd
        else
            unzip -o "${name}.zip"
        fi
        rm -f "${name}.zip"
    )
    echo "[done]  ${name} → ${target_dir}"
}

main() {
    if [[ $# -eq 0 ]]; then
        for name in "${!URLs[@]}"; do
            download_one "${name}" || true
        done
        exit 0
    fi

    case "$1" in
        --list)
            list_corpora
            ;;
        -h|--help)
            cat <<EOF
Usage: $0 [corpus_name|--list]

Downloads test corpora for omnizip-rs benchmarks. Without args,
downloads all. With a name, downloads one.

Available: $(echo "${!URLs[@]}" | tr ' ' '\n' | sort | tr '\n' ' ')
EOF
            ;;
        *)
            for name in "$@"; do
                download_one "${name}"
            done
            ;;
    esac
}

main "$@"
