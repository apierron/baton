use thiserror::Error;

#[derive(Error, Debug)]
pub enum BatonError {
    #[error("Artifact file not found: {0}")]
    ArtifactNotFound(String),

    #[error("Artifact must be a file, not a directory: {0}")]
    ArtifactIsDirectory(String),

    #[error("Context '{name}' path not found: {path}")]
    ContextNotFound { name: String, path: String },

    #[error("Context '{name}' must be a file, not a directory: {path}")]
    ContextIsDirectory { name: String, path: String },

    #[error("Missing required context '{name}' for gate '{gate}'")]
    MissingRequiredContext { name: String, gate: String },

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
}

pub type Result<T> = std::result::Result<T, BatonError>;
