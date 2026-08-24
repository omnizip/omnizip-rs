//! Deterministic-write rules (TODO.containers task 17): the same
//! input tree + options must produce a byte-identical archive across
//! runs, machines, and Rust versions. Normalization lives here;
//! format crates consume [`WriteOptions`], never invent their own.
#![forbid(unsafe_code)]

/// Options controlling archive writing. The [`WriteOptions::
/// deterministic`] constructor applies every normalization rule;
/// the default keeps source metadata.
#[derive(Clone, Debug)]
pub struct WriteOptions {
    /// Fixed mtime for every entry (unix seconds).
    pub mtime: u64,
    /// Normalized uid/gid (0 unless preserving).
    pub uid: u32,
    pub gid: u32,
    /// Normalized uname/gname ("" unless preserving).
    pub uname: String,
    pub gname: String,
    /// File permission bits.
    pub file_mode: u32,
    /// Directory permission bits.
    pub dir_mode: u32,
    /// Host/tool string for format headers that carry one.
    pub host_tool: String,
}

impl WriteOptions {
    /// Fully normalized: mtime from `SOURCE_DATE_EPOCH` (else the
    /// epoch), root ownership, 0644/0755 modes, fixed host string.
    /// This is what the CLI uses by default.
    #[must_use]
    pub fn deterministic() -> Self {
        Self {
            mtime: source_date_epoch(),
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            file_mode: 0o644,
            dir_mode: 0o755,
            host_tool: format!("ozip {}", env!("CARGO_PKG_VERSION")),
        }
    }

    /// Preserve source metadata (explicit opt-out of normalization).
    #[must_use]
    pub fn preserving() -> Self {
        Self {
            mtime: 0,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            file_mode: 0o644,
            dir_mode: 0o755,
            host_tool: format!("ozip {}", env!("CARGO_PKG_VERSION")),
        }
    }

    /// Override the fixed mtime (`--mtime=<t>`).
    #[must_use]
    pub const fn with_mtime(mut self, mtime: u64) -> Self {
        self.mtime = mtime;
        self
    }
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self::deterministic()
    }
}

fn source_date_epoch() -> u64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_is_epoch_zero_without_source_date() {
        // Clear to make the test independent of the environment.
        std::env::remove_var("SOURCE_DATE_EPOCH");
        let o = WriteOptions::deterministic();
        assert_eq!(o.mtime, 0);
        assert_eq!(o.uid, 0);
        assert_eq!(o.file_mode, 0o644);
        assert_eq!(o.dir_mode, 0o755);
        assert!(o.host_tool.starts_with("ozip "));
    }
}
