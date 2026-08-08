# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Dictionary references + zero-literal edge case + default dispatch by @[object]
- Huffman-coded metablock encoder from RFC 7932 by @[object]
- Dictionary hash table infrastructure + find_dictionary_match API by @[object]

### Fixed

- From-spec encoder round-trips + decoder ISLAST=1 fix by @[object]

### Other

- Bump workspace to 0.15.0 by @[object]
- Cargo fmt by @[object]
- Add compression-ratio gates for text and CSV inputs by @[object]
- Cargo fmt by @[object]
- LZ4 faster incompressibility probe + Brotli dictionary lookup API by @[object]
