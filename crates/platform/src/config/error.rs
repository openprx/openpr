//! Errors produced while locating, parsing and validating the configuration file.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::error::AppError;

/// Path of the annotated example shipped with the repository.
pub const EXAMPLE_CONFIG_PATH: &str = "config/openpr.example.toml";

/// Why a configuration file could not be turned into a usable [`super::OpenPrConfig`].
///
/// `Debug` is derived, which is safe because no variant carries a configuration *value*: the
/// parse variant keeps the parser message and a position, never the offending source line, and
/// the validation variant keeps issue descriptions written by this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// No file exists at the requested path.
    NotFound { path: PathBuf },
    /// The file exists but could not be read.
    Unreadable { path: PathBuf, reason: String },
    /// The file is not well formed TOML, or does not match the expected shape.
    Malformed {
        path: PathBuf,
        line: usize,
        column: usize,
        reason: String,
    },
    /// The file parsed but one or more values are unusable.
    Invalid { path: PathBuf, issues: Vec<String> },
}

impl ConfigError {
    /// Builds the [`ConfigError::Malformed`] variant from a TOML deserialization failure.
    ///
    /// The parser's own `Display` renders the offending source line under a caret. That line can
    /// be `jwt_secret = "..."`, so it is deliberately dropped here: only the parser's message and
    /// the position it points at survive into the error.
    pub fn from_toml(path: &Path, source: &str, error: &toml::de::Error) -> Self {
        let (line, column) = error.span().map_or((0, 0), |span| line_and_column(source, span.start));
        Self::Malformed {
            path: path.to_path_buf(),
            line,
            column,
            reason: error.message().trim().to_string(),
        }
    }

    /// The file this error is about.
    pub fn path(&self) -> &Path {
        match self {
            Self::NotFound { path }
            | Self::Unreadable { path, .. }
            | Self::Malformed { path, .. }
            | Self::Invalid { path, .. } => path.as_path(),
        }
    }
}

/// Translates a byte offset into a 1-based line and column.
fn line_and_column(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let consumed = source.get(..offset).unwrap_or(source);
    let line = consumed.matches('\n').count() + 1;
    let column = consumed.rsplit('\n').next().map_or(1, |tail| tail.chars().count() + 1);
    (line, column)
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { path } => write!(
                f,
                "configuration file not found at {}. OpenPR is configured by file only and reads no \
                 environment variables: copy {EXAMPLE_CONFIG_PATH} to {default}, replace every \
                 replace_with_* placeholder (generate secrets with `openssl rand -hex 32`), then start the \
                 service from the directory that holds it or pass --config <path>",
                path.display(),
                default = super::DEFAULT_CONFIG_PATH,
            ),
            Self::Unreadable { path, reason } => {
                write!(f, "configuration file {} could not be read: {reason}", path.display())
            }
            Self::Malformed {
                path,
                line,
                column,
                reason,
            } => write!(
                f,
                "configuration file {} is malformed at line {line}, column {column}: {reason} (the offending \
                 line is not reproduced here because it may hold a secret)",
                path.display()
            ),
            Self::Invalid { path, issues } => {
                writeln!(
                    f,
                    "configuration file {} has {count} unusable {noun}:",
                    path.display(),
                    count = issues.len(),
                    noun = if issues.len() == 1 { "value" } else { "values" }
                )?;
                for issue in issues {
                    writeln!(f, "  - {issue}")?;
                }
                write!(
                    f,
                    "fix all of them in one pass; see {EXAMPLE_CONFIG_PATH} for the expected shape"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<ConfigError> for AppError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error.to_string())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::pedantic,
    clippy::nursery
)]
mod tests {
    use super::{ConfigError, line_and_column};
    use crate::error::AppError;
    use std::path::PathBuf;

    #[test]
    fn not_found_message_names_the_path_and_how_to_create_it() {
        let rendered = ConfigError::NotFound {
            path: PathBuf::from("/srv/openpr/config/openpr.toml"),
        }
        .to_string();
        assert!(rendered.contains("/srv/openpr/config/openpr.toml"), "{rendered}");
        assert!(rendered.contains("config/openpr.example.toml"), "{rendered}");
        assert!(rendered.contains("--config"), "{rendered}");
    }

    #[test]
    fn invalid_message_lists_every_issue() {
        let rendered = ConfigError::Invalid {
            path: PathBuf::from("openpr.toml"),
            issues: vec!["first".to_string(), "second".to_string(), "third".to_string()],
        }
        .to_string();
        assert!(rendered.contains("3 unusable values"), "{rendered}");
        assert!(rendered.contains("- first"), "{rendered}");
        assert!(rendered.contains("- second"), "{rendered}");
        assert!(rendered.contains("- third"), "{rendered}");
    }

    #[test]
    fn malformed_message_keeps_the_position_but_not_the_source_line() {
        let source = "[auth]\njwt_secret = \"top-secret-value\"\n";
        let error = toml::from_str::<toml::Value>("[auth]\njwt_secret = \n").expect_err("should fail");
        let rendered = ConfigError::from_toml(&PathBuf::from("openpr.toml"), source, &error).to_string();
        assert!(!rendered.contains("top-secret-value"), "{rendered}");
        assert!(rendered.contains("line"), "{rendered}");
    }

    #[test]
    fn converts_into_the_platform_error_type() {
        let error: AppError = ConfigError::NotFound {
            path: PathBuf::from("openpr.toml"),
        }
        .into();
        assert!(matches!(error, AppError::Config(_)));
        assert!(error.to_string().contains("openpr.toml"));
    }

    #[test]
    fn byte_offsets_translate_to_one_based_positions() {
        assert_eq!(line_and_column("abc", 0), (1, 1));
        assert_eq!(line_and_column("abc", 2), (1, 3));
        assert_eq!(line_and_column("ab\ncd", 3), (2, 1));
        assert_eq!(line_and_column("ab\ncd", 99), (2, 3));
    }
}
