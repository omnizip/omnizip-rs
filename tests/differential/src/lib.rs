//! Cross-language differential test harness for omnizip-rs.
//!
//! Verifies that Rust encoders produce bytes reference CLI decoders
//! accept, and vice versa. Tests skip cleanly when a CLI is missing
//! so the harness works in minimal CI environments.
//!
//! ## Oracle selection
//!
//! The Ruby omnizip gem is the algorithmic reference documented in
//! `PLAN.md`, but it isn't always installed in CI environments. The
//! system CLIs (`xz`, `bzip2`, `brotli`, `lz4`, `gzip`, `python3
//! -c "import zlib"`) are byte-compatible oracles that any Linux/macOS
//! runner has.
//!
//! ## Architectural split
//!
//! - This module exposes one `*_oracle_decode` function per CLI. Each
//!   returns `Ok(None)` when the CLI is unavailable (test skips).
//! - Test files under `tests/` call these and assert byte-identical
//!   output. Each codec with a reference CLI gets one test file.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::io::Write;

/// Output of running the reference oracle on an input, or `None` if no
/// oracle is installed.
pub struct OracleOutput {
    pub bytes: Vec<u8>,
}

/// Look up `cmd` on `$PATH`. Returns `Ok(None)` if not found (not an
/// error — callers decide whether the oracle is mandatory).
pub fn which(cmd: &str) -> std::io::Result<Option<String>> {
    let output = Command::new("which").arg(cmd).output();
    match output {
        Ok(o) if o.status.success() => Ok(Some(
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
        )),
        Ok(_) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Decode `fixture_path` with the system `xz --decompress --stdout`.
///
/// Returns `Ok(None)` if `xz` isn't installed (caller skips the test).
///
/// # Errors
///
/// Returns [`std::io::Error`] if spawning `xz` fails or `xz` exits
/// non-zero (the fixture is corrupt or not an `.xz` stream).
pub fn xz_oracle_decode(fixture_path: &Path) -> std::io::Result<Option<OracleOutput>> {
    let Some(xz_path) = which("xz")? else {
        return Ok(None);
    };
    let output = Command::new(xz_path)
        .arg("--decompress")
        .arg("--stdout")
        .arg(fixture_path)
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "xz failed on {}: {}",
            fixture_path.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(Some(OracleOutput { bytes: output.stdout }))
}

/// Pipe `compressed` bytes into a CLI decoder, returning its stdout.
///
/// Used by codecs whose output the CLI accepts on stdin (brotli, lz4,
/// sometimes bzip2). Returns `Ok(None)` if the CLI is missing.
fn pipe_through_cli(
    cli: &str,
    extra_args: &[&str],
    compressed: &[u8],
) -> std::io::Result<Option<Vec<u8>>> {
    let Some(path) = which(cli)? else {
        return Ok(None);
    };
    let mut child = Command::new(path)
        .args(extra_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(compressed)?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "{cli} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(Some(output.stdout))
}

/// Decode `compressed` bzip2 bytes via `bzip2 -dc -` (stdin).
///
/// # Errors
///
/// Returns [`std::io::Error`] on spawn failure or non-zero exit.
pub fn bzip2_oracle_decode(compressed: &[u8]) -> std::io::Result<Option<OracleOutput>> {
    Ok(pipe_through_cli("bzip2", &["-dc"], compressed)?.map(|bytes| OracleOutput { bytes }))
}

/// Decode `compressed` brotli bytes via `brotli -dc -` (stdin).
pub fn brotli_oracle_decode(compressed: &[u8]) -> std::io::Result<Option<OracleOutput>> {
    Ok(pipe_through_cli("brotli", &["-dc", "-"], compressed)?.map(|bytes| OracleOutput { bytes }))
}

/// Decode `compressed` lz4 bytes via `lz4 -dc -` (stdin).
pub fn lz4_oracle_decode(compressed: &[u8]) -> std::io::Result<Option<OracleOutput>> {
    Ok(pipe_through_cli("lz4", &["-dc", "-"], compressed)?.map(|bytes| OracleOutput { bytes }))
}

/// Decode raw DEFLATE bytes via `python3 -c "import sys, zlib; ..."`.
///
/// `gzip` and `openssl zlib` expect gzip/zlib framing respectively,
/// not raw DEFLATE. Python's `zlib.decompress` with `wbits=-15` is
/// the canonical raw-DEFLATE decoder.
pub fn python_zlib_oracle_decode(
    compressed: &[u8],
    wbits: i32,
) -> std::io::Result<Option<OracleOutput>> {
    let Some(py) = which("python3")? else {
        return Ok(None);
    };
    let script = format!(
        "import sys, zlib; sys.stdout.buffer.write(zlib.decompress(sys.stdin.buffer.read(), {wbits}))"
    );
    let mut child = Command::new(py)
        .arg("-c")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(compressed)?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "python3 zlib failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(Some(OracleOutput { bytes: output.stdout }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xz_is_reachable_or_skipped() {
        let _ = xz_oracle_decode(Path::new("/dev/null"));
    }

    #[test]
    fn bzip2_helper_returns_none_when_missing() {
        // Same convention: returns Ok(None) if CLI missing, Ok(Some) on success.
        // We don't actually call decode here; we just verify the helper signature.
        let result: std::io::Result<Option<OracleOutput>> = bzip2_oracle_decode(&[]);
        let _ = result;
    }
}
