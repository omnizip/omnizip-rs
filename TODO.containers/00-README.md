# TODO.containers — porting the omnizip archive-container layer to Rust

All remaining work to give omnizip-rs the archive-format layer that today
only exists in the [Ruby gem](https://github.com/omnizip/omnizip), so a
Rust CLI (`ozip`, see [18](18-cli-ozip-v0.md)) can read and write every
format the suite supports. Each file is self-contained: goals, dependencies,
acceptance criteria, and the Ruby → Rust module map.

## Source of truth

Same rule as the codec ports: the Ruby implementations are the
**behavioral reference**. Every Rust container is a line-by-line translation
of the corresponding Ruby file; the cross-language differential gate
([02](../TODO.omnizip-rs/02-cross-language-differential-harness.md),
extended by [20](20-container-differential-harness.md)) and the reference C
tools (`unzip`, `7z`, `unrar`, `bsdtar`) are oracles, in that order.

## Format portfolio

| # | Task | Format | Ruby LOC | Priority | Notes |
|---|---|---|---:|---|---|
| 01 | [01](01-archive-core.md) | core layer | ~1,300 | P0 | Entry, IO, ArchiveHandler, traversal guard |
| 02 | [02](02-tar.md) | TAR | 665 | P0 | POSIX extensions; proves the layer |
| 03 | [03](03-gzip-bzip2-files.md) | GZIP / BZIP2 | 210 | P0 | wrappers over existing codecs |
| 04 | [04](04-zip.md) | ZIP + ZIP64 | 1,658 | P0 | methods 0/8/9 |
| 05 | [05](05-zip-encryption.md) | WinZip AES | (in zip) | P1 | needs a crypto decision |
| 06 | [06](06-sevenzip.md) | 7z | 5,020 | P1 | phased: read → write → solid/volumes/AES |
| 07 | [07](07-rar5.md) | RAR5 | 1,602+core | P1 | read/write, LZMA, volumes, AES-256, PAR2 hooks |
| 08 | [08](08-rar4-read.md) | RAR4 read | 746+core | P2 | all compression methods, read-only |
| 09 | [09](09-cpio-rpm.md) | CPIO + RPM | 936 + 1,282 | P1 | RPM payloads ride the codecs |
| 10 | [10](10-xar.md) | XAR | 2,038 | P2 | XML TOC; needs an XML decision |
| 11 | [11](11-iso9660.md) | ISO 9660 | 2,345 | P2 | Rock Ridge, Joliet |
| 12 | [12](12-ole-msi.md) | OLE + MSI(read) | 2,129 + 1,451 | P2 | MSI reads through OLE + CAB |
| 13 | [13](13-par2.md) | PAR2 | 4,567 | P2 | Reed-Solomon over GF(2^16) |
| 14 | [14](14-lzip-lzma-alone.md) | lzip / .lzma files | small | P2 | already in `omnizip-lzma`; wire as formats |
| 18 | [18](18-cli-ozip-v0.md) | `ozip` v0 | new | **P0, parallel** | codec CLI; no container deps |
| 15 | [15](15-cli-containers.md) | CLI containers | new | P1 | `ozip c/x/t/l` over formats |
| 16 | [16](16-container-benchmarks.md) | bench | new | P1 | archive-level corpora in omnizip-bench |
| 17 | [17](17-determinism-normalization.md) | determinism | new | P0 | normalized metadata, byte-identical archives |
| 20 | [20](20-container-differential-harness.md) | differential | extend | P0 | fixtures + cross-tool oracles |
| 21 | [21](21-security-hardening.md) | security | new | P0 | traversal, symlinks, bombs, absolute paths |

## Standing rules

1. **Determinism is a container property too.** Byte-identical archives for
   the same input tree + options, across runs and machines — see
   [17](17-determinism-normalization.md). This is the CLI's headline feature.
2. **Security at the extraction boundary** — path traversal, symlink escapes,
   zip bombs, absolute paths: [21](21-security-hardening.md), enforced in
   `01-archive-core`, not per-format.
3. **One crate per format family** (`omnizip-tar`, `omnizip-zip`, …) behind a
   shared `ArchiveReader`/`ArchiveWriter` trait pair in `omnizip-archive-core`
   — the same registry pattern the codecs use.
4. **No unsafe, no C.** Crypto and XML are the two places this gets tempting;
   both are explicit decision tasks ([05](05-zip-encryption.md),
   [10](10-xar.md)).
