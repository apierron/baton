//! Error types for the baton crate.

use thiserror::Error;

/// All errors that can occur during baton operation.
#[derive(Error, Debug)]
pub enum BatonError {
    #[error("{0}")]
    ConfigError(String),

    #[error("TOML parse error: {0}")]
    TomlError(#[from] toml::de::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Gate not found: '{name}'. Available gates: {available}")]
    GateNotFound { name: String, available: String },

    #[error("{0}")]
    ValidationError(String),

    #[error("Unresolved variable '${{{var}}}' in {location}")]
    UnresolvedVariable { var: String, location: String },

    #[error("{0}")]
    PromptError(String),

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Runtime error: {0}")]
    RuntimeError(String),
}

/// Convenience alias for `Result<T, BatonError>`.
pub type Result<T> = std::result::Result<T, BatonError>;
