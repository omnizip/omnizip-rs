//! Password layer — port of the Ruby gem's `password/` subsystem
//! (`TODO.ref-parity/54`).
//!
//! [`PasswordProvider`] resolves the password for an archive open;
//! [`PasswordValidator`] ports `password_validator.rb`'s policy and
//! strength scoring for write paths. Callers wire the resolved
//! string into the per-format `from_bytes_with_password` APIs.
//!
//! Design note: the provider resolves ONCE at the open boundary —
//! readers keep plain `&str` parameters. This matches the Ruby
//! layer, where the password provider resolves before the
//! encryption strategy sees the bytes.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use crate::error::ArchiveError;

/// What the provider is being asked for.
#[derive(Debug, Clone)]
pub struct PasswordPrompt {
    /// Archive being opened (path or name).
    pub archive: String,
    /// Encrypted entry, when the caller knows it.
    pub entry: Option<String>,
    /// Encryption scheme ("7z-aes", "winzip-aes", "rar3", "rar5", ...).
    pub scheme: &'static str,
}

impl PasswordPrompt {
    #[must_use]
    pub fn new(archive: impl Into<String>, scheme: &'static str) -> Self {
        Self {
            archive: archive.into(),
            entry: None,
            scheme,
        }
    }
}

/// Resolves a password at an archive-open boundary.
pub trait PasswordProvider {
    /// `Ok(None)` = no password available (the open proceeds
    /// unencrypted or fails with a missing-password error, as the
    /// format dictates).
    ///
    /// # Errors
    ///
    /// Implementations (env/file) return
    /// [`ArchiveError::UnsupportedFeature`] when their source is
    /// unavailable.
    fn password_for(&self, prompt: &PasswordPrompt) -> Result<Option<String>, ArchiveError>;
}

/// The current behavior: one fixed password (or none).
#[derive(Debug, Clone, Default)]
pub struct StaticProvider {
    password: Option<String>,
}

impl StaticProvider {
    #[must_use]
    pub fn new(password: Option<String>) -> Self {
        Self { password }
    }
}

impl PasswordProvider for StaticProvider {
    fn password_for(&self, _prompt: &PasswordPrompt) -> Result<Option<String>, ArchiveError> {
        Ok(self.password.clone())
    }
}

/// Reads the password from an environment variable.
#[derive(Debug, Clone)]
pub struct EnvProvider {
    pub var: String,
}

impl PasswordProvider for EnvProvider {
    fn password_for(&self, _prompt: &PasswordPrompt) -> Result<Option<String>, ArchiveError> {
        Ok(std::env::var(&self.var).ok())
    }
}

/// Reads the first line of a file as the password.
#[derive(Debug, Clone)]
pub struct FileProvider {
    pub path: PathBuf,
}

impl PasswordProvider for FileProvider {
    fn password_for(&self, _prompt: &PasswordPrompt) -> Result<Option<String>, ArchiveError> {
        let raw =
            std::fs::read_to_string(&self.path).map_err(|e| ArchiveError::UnsupportedFeature {
                reason: format!("password file {}: {e}", self.path.display()),
            })?;
        Ok(raw.lines().next().map(ToOwned::to_owned))
    }
}

/// Closure-based provider — the seam interactive UIs (ozip prompts,
/// the future Ruby bridge) plug into.
pub struct FnProvider<F>
where
    F: Fn(&PasswordPrompt) -> Result<Option<String>, ArchiveError> + Send + Sync,
{
    f: F,
}

impl<F> FnProvider<F>
where
    F: Fn(&PasswordPrompt) -> Result<Option<String>, ArchiveError> + Send + Sync,
{
    #[must_use]
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

impl<F> PasswordProvider for FnProvider<F>
where
    F: Fn(&PasswordPrompt) -> Result<Option<String>, ArchiveError> + Send + Sync,
{
    fn password_for(&self, prompt: &PasswordPrompt) -> Result<Option<String>, ArchiveError> {
        (self.f)(prompt)
    }
}

/// Port of `password_validator.rb`: policy checks + strength score.
#[derive(Debug, Clone)]
pub struct PasswordValidator {
    pub min_length: usize,
    pub require_uppercase: bool,
    pub require_lowercase: bool,
    pub require_numbers: bool,
    pub require_special: bool,
}

impl Default for PasswordValidator {
    fn default() -> Self {
        Self {
            min_length: 8,
            require_uppercase: false,
            require_lowercase: false,
            require_numbers: false,
            require_special: false,
        }
    }
}

impl PasswordValidator {
    /// `Err(reason)` when the password violates the policy (the
    /// Ruby `validate` semantics, without the exception).
    ///
    /// # Errors
    ///
    /// [`ArchiveError::Security`] with the specific rule violated.
    pub fn validate(&self, password: &str) -> Result<(), ArchiveError> {
        if password.is_empty() {
            return Err(ArchiveError::Security("password cannot be empty".into()));
        }
        if password.len() < self.min_length {
            return Err(ArchiveError::Security(format!(
                "password too short (minimum: {} characters)",
                self.min_length
            )));
        }
        if self.require_uppercase && !password.chars().any(|c| c.is_ascii_uppercase()) {
            return Err(ArchiveError::Security(
                "password must contain uppercase letters".into(),
            ));
        }
        if self.require_lowercase && !password.chars().any(|c| c.is_ascii_lowercase()) {
            return Err(ArchiveError::Security(
                "password must contain lowercase letters".into(),
            ));
        }
        if self.require_numbers && !password.chars().any(|c| c.is_ascii_digit()) {
            return Err(ArchiveError::Security(
                "password must contain numbers".into(),
            ));
        }
        if self.require_special
            && !password
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || c == '_'))
        {
            return Err(ArchiveError::Security(
                "password must contain special characters".into(),
            ));
        }
        Ok(())
    }

    /// Strength score 0-100: length up to 40, character variety up
    /// to 60 (the Ruby `strength` formula).
    #[must_use]
    pub fn strength(&self, password: &str) -> u8 {
        if password.is_empty() {
            return 0;
        }
        let mut score: usize = (password.len() * 4).min(40);
        if password.chars().any(|c| c.is_ascii_lowercase()) {
            score += 15;
        }
        if password.chars().any(|c| c.is_ascii_uppercase()) {
            score += 15;
        }
        if password.chars().any(|c| c.is_ascii_digit()) {
            score += 15;
        }
        if password
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '_'))
        {
            score += 15;
        }
        score.min(100) as u8
    }

    /// `weak | fair | good | strong` per the Ruby thresholds.
    #[must_use]
    pub fn strength_label(&self, password: &str) -> &'static str {
        let s = self.strength(password);
        match s {
            0..=39 => "weak",
            40..=64 => "fair",
            65..=79 => "good",
            _ => "strong",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt() -> PasswordPrompt {
        PasswordPrompt::new("a.7z", "7z-aes")
    }

    #[test]
    fn static_provider_mirrors_current_behavior() {
        assert_eq!(
            StaticProvider::new(None).password_for(&prompt()).unwrap(),
            None
        );
        assert_eq!(
            StaticProvider::new(Some("pw".into()))
                .password_for(&prompt())
                .unwrap(),
            Some("pw".into())
        );
    }

    #[test]
    fn env_provider_reads_variable() {
        let name = "OMNIZIP_TEST_PW";
        std::env::remove_var(name);
        assert_eq!(
            EnvProvider { var: name.into() }
                .password_for(&prompt())
                .unwrap(),
            None
        );
        std::env::set_var(name, "secret");
        assert_eq!(
            EnvProvider { var: name.into() }
                .password_for(&prompt())
                .unwrap(),
            Some("secret".into())
        );
        std::env::remove_var(name);
    }

    #[test]
    fn file_provider_reads_first_line() {
        let dir = std::env::temp_dir().join(format!("ozip-pw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pw.txt");
        std::fs::write(&path, "first\nsecond\n").unwrap();
        let pw = FileProvider { path }.password_for(&prompt()).unwrap();
        assert_eq!(pw, Some("first".into()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fn_provider_is_the_interactive_seam() {
        let p = FnProvider::new(|prompt: &PasswordPrompt| {
            Ok(Some(format!("pw-for-{}", prompt.scheme)))
        });
        assert_eq!(
            p.password_for(&prompt()).unwrap(),
            Some("pw-for-7z-aes".into())
        );
    }

    #[test]
    fn validator_ports_the_ruby_rules() {
        let v = PasswordValidator::default();
        assert!(v.validate("longenough").is_ok());
        let err = v.validate("short").unwrap_err().to_string();
        assert!(err.contains("too short"), "{err}");
        assert!(v
            .validate("")
            .unwrap_err()
            .to_string()
            .contains("cannot be empty"));

        let strict = PasswordValidator {
            require_uppercase: true,
            require_numbers: true,
            ..PasswordValidator::default()
        };
        assert!(strict.validate("abcdefgh1").is_err());
        assert!(strict.validate("Abcdefgh1").is_ok());
    }

    #[test]
    fn strength_matches_the_ruby_formula() {
        let v = PasswordValidator::default();
        assert_eq!(v.strength(""), 0);
        assert_eq!(v.strength("aaaaaaaaaaaaaaaaaaaa"), 55);
        assert_eq!(v.strength("Password1!"), 100);
        assert_eq!(v.strength("password"), 47);
        assert_eq!(v.strength_label("Password1!"), "strong");
        assert_eq!(v.strength_label("aaaaaaaaaaaaaaaaaaaa"), "fair");
    }
}
