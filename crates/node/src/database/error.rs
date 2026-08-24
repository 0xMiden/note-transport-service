use thiserror::Error;

/// Database-specific error types
#[derive(Debug, Error)]
pub enum DatabaseError {
    /// Configuration error
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// Connection error
    #[error("Connection error: {0}")]
    Connection(String),

    /// Migration error
    #[error("Migration error: {0}")]
    Migration(String),

    /// Query execution error
    #[error("Query execution error: {0}")]
    QueryExecution(String),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Deserialization error
    #[error("Deserialization error: {0}")]
    Deserialization(String),
}
