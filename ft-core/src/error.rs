use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("vault not found; searched:\n{}", .tried.join("\n"))]
    VaultNotFound { tried: Vec<String> },

    #[error("config error in {path}: {source}")]
    Config {
        path: String,
        source: Box<figment::Error>,
    },

    #[error("I/O error at {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("notes: {0}")]
    Notes(String),

    #[error("periodic notes: {0}")]
    Periodic(String),

    #[error("git: {0}")]
    Git(String),

    #[error("timeblock: {0}")]
    Timeblock(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// A non-fatal error encountered while scanning one file. Collected in
/// [`Scan::errors`] rather than aborting the whole scan.
#[derive(Debug)]
pub struct ScanError {
    /// Vault-relative path of the offending file (or absolute if it sits
    /// outside the vault root).
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}
