// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! Error types for the tigrs application and engine.

use std::path::PathBuf;
use thiserror::Error;

/// The primary error type for all tigrs operations.
#[derive(Debug, Error)]
pub enum TigError {
    /// An I/O error occurred during file or descriptor operations.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A Git engine error occurred during object retrieval, revwalk, or diffing.
    #[error("Git error: {0}")]
    Git(String),

    /// A Git engine error with an underlying source error preserved.
    #[error("Git error: {context}: {source}")]
    GitWithSource {
        /// Contextual description of the Git operation that failed.
        context: String,
        /// The underlying source error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// Repository discovery failed or directory is not a valid git repository.
    #[error("Repository not found at '{0}'")]
    RepoNotFound(PathBuf),

    /// Configuration parsing or validation failed.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Terminal interaction, TTY initialization, or Crossterm rendering failed.
    #[error("Terminal error: {0}")]
    Terminal(String),

    /// Operation was cancelled due to user navigation or generation supersedure.
    #[error("Operation cancelled")]
    Cancelled,

    /// Command macro interpolation or shell dispatch failed.
    #[error("Command error: {0}")]
    Command(String),

    /// An invalid argument was supplied to a function or CLI flag.
    #[error("Invalid argument: {0}")]
    Argument(String),

    /// An internal engine or runtime failure occurred.
    #[error("Internal error: {0}")]
    Internal(String),

    /// A security policy or path traversal violation occurred.
    #[error("Security error: {0}")]
    Security(String),

    /// A repository mutation was attempted while in read-only mode.
    #[error("Read-only mode: {0} (launch with --update-mode or run ':set read-only = false')")]
    ReadOnly(String),
}

/// A specialized Result type for tigrs operations.
pub type Result<T> = std::result::Result<T, TigError>;

impl TigError {
    /// Constructs a structured `TigError::GitWithSource` preserving the underlying error cause.
    pub fn git<E>(context: impl Into<String>, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::GitWithSource {
            context: context.into(),
            source: Box::new(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn test_error_display_formatting() {
        assert_eq!(
            TigError::Git("failed revwalk".to_string()).to_string(),
            "Git error: failed revwalk"
        );
        assert_eq!(
            TigError::RepoNotFound(PathBuf::from("/nonexistent")).to_string(),
            "Repository not found at '/nonexistent'"
        );
        assert_eq!(
            TigError::Config("invalid toml".to_string()).to_string(),
            "Configuration error: invalid toml"
        );
        assert_eq!(
            TigError::Terminal("raw mode failed".to_string()).to_string(),
            "Terminal error: raw mode failed"
        );
        assert_eq!(TigError::Cancelled.to_string(), "Operation cancelled");
        assert_eq!(
            TigError::Command("macro expansion".to_string()).to_string(),
            "Command error: macro expansion"
        );
        assert_eq!(
            TigError::Argument("unknown flag".to_string()).to_string(),
            "Invalid argument: unknown flag"
        );
        assert_eq!(
            TigError::Internal("arena corruption".to_string()).to_string(),
            "Internal error: arena corruption"
        );
        assert_eq!(
            TigError::Security("symlink escape".to_string()).to_string(),
            "Security error: symlink escape"
        );
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file missing");
        let tig_err: TigError = io_err.into();
        assert!(matches!(tig_err, TigError::Io(_)));
        assert!(tig_err.to_string().contains("I/O error: file missing"));

        let perm_err = io::Error::new(io::ErrorKind::PermissionDenied, "access denied");
        let tig_perm: TigError = perm_err.into();
        assert!(matches!(tig_perm, TigError::Io(_)));
        assert!(tig_perm.to_string().contains("access denied"));

        let broken_pipe = io::Error::new(io::ErrorKind::BrokenPipe, "pipe closed");
        let tig_pipe: TigError = broken_pipe.into();
        assert!(matches!(tig_pipe, TigError::Io(_)));
        assert!(tig_pipe.to_string().contains("pipe closed"));
    }

    #[test]
    fn test_error_formatting_edge_cases() {
        // Empty strings in errors
        assert_eq!(TigError::Git(String::new()).to_string(), "Git error: ");
        assert_eq!(
            TigError::Config(String::new()).to_string(),
            "Configuration error: "
        );
        assert_eq!(
            TigError::Security(String::new()).to_string(),
            "Security error: "
        );

        // Unicode and special characters
        assert_eq!(
            TigError::RepoNotFound(PathBuf::from("/repo/路径 with spaces/🦀")).to_string(),
            "Repository not found at '/repo/路径 with spaces/🦀'"
        );
        assert_eq!(
            TigError::Command("`rm -rf /` $(reboot) 'quoted'".to_string()).to_string(),
            "Command error: `rm -rf /` $(reboot) 'quoted'"
        );
    }

    #[test]
    fn test_result_type_alias_propagation() {
        fn fallible(val: usize) -> Result<usize> {
            if val > 0 {
                Ok(val)
            } else {
                Err(TigError::Cancelled)
            }
        }

        fn fail_io() -> Result<usize> {
            Err(io::Error::other("custom io failure").into())
        }

        assert_eq!(fallible(42).unwrap(), 42);
        assert!(matches!(fallible(0), Err(TigError::Cancelled)));
        assert!(matches!(fail_io(), Err(TigError::Io(_))));
    }
}
