# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- EOF-checked inflate + 1032:1 expansion cap close a decompression bomb by @[object]

### Fixed

- Exact chunk-tail rule closes the last xz corpus case by @[object]

### Fixed

- Wasm32-safe hint growth in the unknown-length inflate by @[object]
- Xz container validation + zstd dictionary-ID gate by @[object]
