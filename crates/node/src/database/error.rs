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

    /// Unique constraint violation
    ///
    /// Kept separate from [`DatabaseError::ConstraintViolation`] so that callers can tell "this
    /// row already exists" apart from a genuine integrity failure.
    #[error("Unique constraint violation: {0}")]
    UniqueViolation(String),

    /// Constraint violation error
    #[error("Constraint violation: {0}")]
    ConstraintViolation(String),

    /// Transaction error
    #[error("Transaction error: {0}")]
    Transaction(String),

    /// Pool error
    #[error("Connection pool error: {0}")]
    Pool(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

impl From<diesel::result::Error> for DatabaseError {
    fn from(err: diesel::result::Error) -> Self {
        match err {
            diesel::result::Error::DatabaseError(kind, info) => match kind {
                diesel::result::DatabaseErrorKind::UniqueViolation => {
                    Self::UniqueViolation(info.message().to_string())
                },
                diesel::result::DatabaseErrorKind::ForeignKeyViolation => {
                    Self::ConstraintViolation(format!(
                        "Foreign key constraint violation: {}",
                        info.message()
                    ))
                },
                diesel::result::DatabaseErrorKind::NotNullViolation => Self::ConstraintViolation(
                    format!("Not null constraint violation: {}", info.message()),
                ),
                _ => Self::QueryExecution(format!("Database error: {}", info.message())),
            },
            diesel::result::Error::NotFound => Self::QueryExecution("Record not found".to_string()),
            diesel::result::Error::RollbackTransaction => {
                Self::Transaction("Transaction was rolled back".to_string())
            },
            diesel::result::Error::AlreadyInTransaction => {
                Self::Transaction("Already in transaction".to_string())
            },
            diesel::result::Error::NotInTransaction => {
                Self::Transaction("Not in transaction".to_string())
            },
            _ => Self::QueryExecution(format!("Diesel error: {err}")),
        }
    }
}

impl From<deadpool_diesel::PoolError> for DatabaseError {
    fn from(err: deadpool_diesel::PoolError) -> Self {
        Self::Pool(format!("Connection pool error: {err}"))
    }
}

impl From<deadpool_diesel::InteractError> for DatabaseError {
    fn from(err: deadpool_diesel::InteractError) -> Self {
        Self::Connection(format!("Connection interaction error: {err}"))
    }
}

impl From<diesel_migrations::MigrationError> for DatabaseError {
    fn from(err: diesel_migrations::MigrationError) -> Self {
        Self::Migration(format!("Migration error: {err}"))
    }
}
