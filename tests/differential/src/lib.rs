//! Cross-language differential test harness for omnizip-rs.
//!
//! Verifies that Rust decoders produce byte-identical output to a
//! reference oracle on every fixture under `tests/fixtures/`.
//!
//! ## Oracle selection
//!
//! The Ruby omnizip gem is the algorithmic reference documented in
//! `PLAN.md`, but it isn't always installed in CI environments. The
//! system `xz` binary (XZ Utils) is byte-for-byte compatible with the
//! Ruby's expected output for `.lzma` / `.xz` fixtures (the Ruby was
//! ported from XZ Utils), so it serves as a portable oracle.
//!
//! Tests skip cleanly when no oracle is available.

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]

use std::path::Path;
use std::process::Command;

/// Output of running the reference oracle on a fixture, or `None` if no
/// oracle is installed.
pub struct OracleOutput {
    pub bytes: Vec<u8>,
}

/// Decode `fixture_path` with the system `xz --decompress --stdout`.
///
/// Returns `Ok(None)` if `xz` isn't installed (caller should skip the
/// test). Returns `Err` on invocation failure or non-zero exit.
pub fn xz_oracle_decode(fixture_path: &Path) -> std::io::Result<Option<OracleOutput>> {
    let xz = which("xz")?;
    if xz.is_none() {
        return Ok(None);
    }
    let output = Command::new(xz.unwrap())
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
    Ok(Some(OracleOutput {
        bytes: output.stdout,
    }))
}

/// Look up `cmd` on `$PATH`. Returns `Ok(None)` if not found (not an
/// error — callers decide whether the oracle is mandatory).
fn which(cmd: &str) -> std::io::Result<Option<String>> {
    let output = Command::new("which")
        .arg(cmd)
        .output();
    match output {
        Ok(o) if o.status.success() => Ok(Some(
            String::from_utf8_lossy(&o.stdout).trim().to_string(),
        )),
        Ok(_) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xz_is_reachable_or_skipped() {
        // Sanity: either xz is installed (oracle usable) or the test
        // environment has no oracle. Both are valid CI states.
        let result = xz_oracle_decode(Path::new("/dev/null"));
        let _ = result; // `Ok(Some(_))` won't happen for /dev/null; we just want no panic.
    }
}
