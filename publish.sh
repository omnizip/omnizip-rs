#!/bin/bash
# Publish all omnizip-rs crates to crates.io.
# Run this after the 24-hour rate limit resets.
set -e

CRATES=(
    omnizip-codecs
    omnizip-filters
    omnizip-lzma
    omnizip-zstd
    omnizip-flac
    omnizip-snappy
    omnizip-lz4
    omnizip-deflate
    omnizip-brotli
    omnizip-fsst
    omnizip-ricepp
    omnizip-blosc
    omnizip-glza
    omnizip-ppmd
    omnizip-zpaq
)

echo "Publishing ${#CRATES[@]} crates..."
for crate in "${CRATES[@]}"; do
    echo -n "  $crate... "
    if cargo publish -p "$crate" 2>&1 | grep -q "Published"; then
        echo "OK"
    else
        echo "FAILED (see above)"
    fi
done
echo "Done."
