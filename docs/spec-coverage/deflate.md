# Spec Coverage — omnizip-deflate

**Spec:** RFC 1950 (zlib), RFC 1951 (DEFLATE), RFC 1952 (gzip)
**Last updated:** 2026-08-03

## Coverage matrix

| Section | Clause | Description | Test file | Status |
|---------|--------|-------------|-----------|--------|
| RFC 1951 §3.2.3 | Block types | 00=stored, 01=fixed, 10=dynamic | deflate/src/lib.rs | ✅ |
| RFC 1951 §3.2.5 | Fixed Huffman table | Standard code lengths | deflate/src/lib.rs | ✅ |
| RFC 1951 §3.2.6 | Dynamic Huffman table | HLIT/HDIST/HCLEN encoding | deflate/src/lib.rs | ✅ |
| RFC 1951 §3.2.4 | LZ77 back-references | Length/distance encoding | deflate/src/lib.rs | ✅ |
| RFC 1951 §3.2.4 | Length codes 257-285 | Extra bits for lengths | deflate/src/lib.rs | ✅ |
| RFC 1951 §3.2.4 | Distance codes 0-29 | Extra bits for distances | deflate/src/lib.rs | ✅ |
| RFC 1950 §2.2 | zlib header (CMF/FLG) | 78 9C = deflate, default | deflate/src/lib.rs | ✅ |
| RFC 1950 §2.2 | zlib Adler-32 checksum | Appended after DEFLATE | deflate/src/lib.rs | ✅ |
| — | INTEROP | Python zlib.decompress(data) | tests/differential/tests/cli_parity.rs | ✅ |

## Notes

Our encoder wraps miniz_oxide output in zlib format (RFC 1950). The
DEFLATE stream itself (RFC 1951) is produced by miniz_oxide. We add
the zlib header (CMF=0x78, FLG=0x9C) and Adler-32 footer.

## Gaps

1. **gzip format (RFC 1952)**: not implemented. Our encoder produces
   zlib-wrapped DEFLATE only. Some callers want gzip format (with
   CRC-32 and filename fields). **Priority: low**.
2. **Raw DEFLATE**: not directly exposed. Callers who want raw
   DEFLATE (no zlib header) would need to skip the first 2 bytes and
   last 4 bytes. **Priority: low**.
