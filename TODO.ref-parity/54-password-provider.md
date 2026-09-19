# Task 54 — password provider abstraction

Status: open (part of task 51 phase 1)

## Gap

Ruby: `password/` — `encryption_registry` + `encryption_strategy`
(password-derived key handling per scheme), `password_validator`,
`winzip_aes_strategy`, `zip_crypto_strategy`. A unified, swappable
password layer covering BOTH legacy ZipCrypto and WinZip AES.
Rust today: `&str` password params threaded into zip-AES / 7z /
RAR3/RAR5 open paths; ozip `-p <string>` only; no provider concept,
no validator.

## Scope

- `trait PasswordProvider { fn password(&self, prompt: &PasswordPrompt)
  -> Result<Vec<u8>, OmnizipError>; }` in omnizip-codecs (or
  archive-core — placement decision: next to the archive open API,
  i.e. archive-core).
- Providers: `Static` (today's `-p`), `Prompt` (stdin hidden read —
  console interaction lives in ozip, library takes a closure
  provider), `Env`, `File`.
- `PasswordValidator` port: strength/charset rules from
  `password_validator.rb`, used before ENCRYPTED WRITE paths.
- Wire into: zip WinZip AES read/write, 7z AES, RAR3/RAR5 entry +
  header encryption. NOTE while wiring: check whether legacy
  ZipCrypto READ exists in omnizip-zip — the Ruby gem has a
  `zip_crypto_strategy`; if we lack ZipCrypto decode, that is a
  codec gap to file separately (ZipCrypto is tiny; in-house pure
  Rust per policy).
- ozip: `-p` becomes `Static`; add `--password-prompt`,
  `--password-file`.

## Acceptance

- All existing encrypted-fixture tests keep passing through the
  provider indirection (byte-identical decrypts).
- New: env/file provider tests; prompt provider via scripted stdin.
- Validator parity on the Ruby test vectors.

## References

`../omnizip/lib/omnizip/password/*.rb`.
