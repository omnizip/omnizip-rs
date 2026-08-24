# 05 — ZIP encryption (WinZip AES) + the crypto decision

- **Priority:** P1
- **Depends on:** [04](04-zip.md)
- **Estimated effort:** 1–2 weeks (after the decision)
- **Crate:** `omnizip-zip` (+ crypto module location per decision)

## The decision first

WinZip AES needs AES-CTR + HMAC-SHA1/SHA2 + PBKDF2 (1000 iters, per spec).
7z needs AES-CBC + SHA-256 key derivation. RAR5 needs AES-CBC + PBKDF2-HMAC-SHA256.
PAR2 needs MD5 + SHA-1 + Reed-Solomon.

**Options:**
(a) in-house pure-Rust implementations (largest LOC, most review burden),
(b) vetted pure-Rust crates (`aes`, `sha2`, `pbkdf2`, `hmac` — all RustCrypto,
    safe, no unsafe in their core) as workspace dependencies.

**Recommendation:** (b). The codecs' "no dependencies" rule exists to keep
the wire-format crates minimal; crypto is a different domain where
hand-rolling is exactly the kind of code we should not write. Gate: RustCrypto
crates only, pinned, audited versions, and they live in a separate
`omnizip-crypto` crate so codec crates stay dependency-free.

## Goal

WinZip AES (AE-1/AE-2) read + write: key derivation, per-file salt + password
verification bytes, HMAC authentication.

## Ruby → Rust module map

| Ruby source | Rust module | Notes |
|---|---|---|
| `crypto/` (AES, PBKDF2, HMAC) | `omnizip-crypto` | depends on the decision above |
| zip writer/reader AES branches | `zip/aes.rs` | spec: WinZip AES encryption, AE-2 |

## Acceptance

- [ ] Round-trip + `unzip -P` decodes our AES archives; we decode WinZip's
- [ ] Wrong-password path fails on the verification bytes, never on padding
- [ ] Test vectors from the WinZip AES APPNOTE
