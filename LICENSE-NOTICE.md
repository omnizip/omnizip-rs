# License Notice

This workspace is dual-licensed **MIT OR Apache-2.0** (see `LICENSE-MIT`
and `LICENSE-APACHE`).

## Ruby reference — omnizip

The Rust modules in this workspace are line-by-line ports of the Ruby
implementations in [`omnizip/omnizip`](https://github.com/omnizip/omnizip).
Each Ruby source file carries the header:

```
Copyright (C) 2025 Ribose Inc.
Permission is hereby granted, free of charge, ...
```

(MIT). The Rust ports inherit MIT compatibility.

## C reference (consulted for performance tuning only)

- LZMA / XZ: [`tukaani-project/xz`](https://github.com/tukaani-project/xz)
  liblzma — 0BSD / public domain.
- ZSTD: [`facebook/zstd`](https://github.com/facebook/zstd) — BSD-3-Clause.

The C source is **not** the porting basis; the Ruby is. C is consulted
after the Ruby port verifies correct, only when optimising hot paths.
