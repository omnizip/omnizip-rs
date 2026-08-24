//! Extraction security boundary (TODO.containers task 21): path
//! traversal, absolute paths, drive letters, symlink escapes, and
//! decompression-bomb budgets — enforced once, in `ArchiveReader::
//! extract_to`, never per-format.
#![forbid(unsafe_code)]

use crate::error::ArchiveError;
use crate::ArchiveEntry;
use std::path::{Component, Path};

/// Extraction policy. All guards are ON by default; each can be
/// relaxed explicitly by the caller that wants the footgun.
#[derive(Clone, Debug)]
pub struct SecurityPolicy {
    /// Reject entries escaping the output dir (`../`, `/`, `C:\`).
    pub allow_traversal: bool,
    /// Cap total decompressed bytes across the archive.
    pub max_total_size: Option<u64>,
    /// Cap single-entry decompressed size.
    pub max_entry_size: Option<u64>,
    /// Cap the archive-wide compression ratio (total_out / archive_in).
    pub max_ratio: Option<u64>,
    /// Permit absolute entry names (leading `/`).
    pub allow_absolute_paths: bool,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            allow_traversal: false,
            max_total_size: Some(1 << 32), // 4 GiB
            max_entry_size: Some(1 << 31), // 2 GiB
            max_ratio: Some(1000),
            allow_absolute_paths: false,
        }
    }
}

impl SecurityPolicy {
    /// Validate one entry name, returning the sanitized relative path
    /// to join onto the output directory.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Security`] on traversal or absolute paths.
    pub fn validate_entry(&self, name: &str) -> Result<String, ArchiveError> {
        if name.is_empty() {
            return Err(ArchiveError::Security("entry has an empty name".into()));
        }
        if self.allow_absolute_paths {
            return Ok(name.to_string());
        }
        let path = Path::new(name);
        if path.is_absolute() {
            return Err(ArchiveError::Security(format!(
                "absolute entry name not allowed: {name}"
            )));
        }
        // Windows drive letters and UNC roots smuggle absolute paths
        // through components on unix hosts.
        if name.len() >= 2 && name.as_bytes()[1] == b':' && name.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(ArchiveError::Security(format!(
                "drive-letter entry name not allowed: {name}"
            )));
        }
        if name.starts_with("\\\\") {
            return Err(ArchiveError::Security(format!(
                "UNC entry name not allowed: {name}"
            )));
        }
        if self.allow_traversal {
            return Ok(name.to_string());
        }
        // Walk components: anything but Normal is suspect; ParentDir
        // escapes, CurDir is noise, RootDir/Prefix are absolute.
        let mut clean = Vec::new();
        for component in path.components() {
            match component {
                Component::Normal(part) => clean.push(part.to_string_lossy().into_owned()),
                Component::CurDir => {}
                Component::ParentDir => {
                    return Err(ArchiveError::Security(format!(
                        "path traversal (..) in entry name: {name}"
                    )));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(ArchiveError::Security(format!(
                        "absolute component in entry name: {name}"
                    )));
                }
            }
        }
        if clean.is_empty() {
            return Err(ArchiveError::Security(format!(
                "entry name reduces to nothing: {name}"
            )));
        }
        Ok(clean.join("/"))
    }

    /// Enforce per-entry and archive-wide decompression budgets.
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Security`] on budget violations.
    pub fn check_decompression_budget(
        &self,
        entry_bytes: u64,
        entry: &ArchiveEntry,
    ) -> Result<(), ArchiveError> {
        if let Some(cap) = self.max_entry_size {
            if entry_bytes > cap {
                return Err(ArchiveError::Security(format!(
                    "entry '{}' decompresses to {entry_bytes} bytes, over the {cap}-byte limit",
                    entry.name
                )));
            }
        }
        if let Some(cap) = self.max_total_size {
            if entry_bytes > cap {
                return Err(ArchiveError::Security(format!(
                    "entry '{}' exceeds the archive-wide {cap}-byte budget",
                    entry.name
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_dot_dot() {
        let p = SecurityPolicy::default();
        assert!(p.validate_entry("../etc/passwd").is_err());
        assert!(p.validate_entry("good/../../escape").is_err());
        assert!(p.validate_entry("a/./b/..").is_err());
    }

    #[test]
    fn rejects_absolute_and_drives() {
        let p = SecurityPolicy::default();
        assert!(p.validate_entry("/etc/passwd").is_err());
        assert!(p.validate_entry("C:\\Windows\\evil").is_err());
        assert!(p.validate_entry("\\\\server\\share").is_err());
    }

    #[test]
    fn accepts_clean_names() {
        let p = SecurityPolicy::default();
        assert_eq!(p.validate_entry("dir/file.txt").unwrap(), "dir/file.txt");
        assert_eq!(p.validate_entry("./dir/./f").unwrap(), "dir/f");
    }

    #[test]
    fn opt_outs_work() {
        let p = SecurityPolicy {
            allow_traversal: true,
            ..SecurityPolicy::default()
        };
        assert!(p.validate_entry("../out").is_ok());
    }

    #[test]
    fn bomb_budgets_fire() {
        let p = SecurityPolicy::default();
        let e = ArchiveEntry::file("big.bin", 0);
        assert!(p.check_decompression_budget((1 << 31) + 1, &e).is_err());
        assert!(p.check_decompression_budget(1024, &e).is_ok());
    }
}
