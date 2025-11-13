//! Database Error Types
//!
//! This module defines error types for database operations, providing
//! clear error handling for connection, initialization, and query failures.

use std::path::PathBuf;
use thiserror::Error;

/// Database operation errors
///
/// Covers all error cases for database connection, initialization,
/// and basic operations. More specific query errors are handled by
/// service-layer error types.
#[derive(Error, Debug)]
pub enum DatabaseError {
    /// Failed to establish database connection
    #[error("Failed to connect to database at {path}: {source}")]
    ConnectionFailed {
        path: PathBuf,
        source: libsql::Error,
    },

    /// Failed to initialize database schema
    #[error("Failed to initialize database schema: {0}")]
    InitializationFailed(String),

    /// Invalid database path provided
    #[error("Invalid database path: {path}")]
    InvalidPath { path: PathBuf },

    /// Permission denied when accessing database
    #[error("Permission denied for database path: {path}")]
    PermissionDenied { path: PathBuf },

    /// Failed to create parent directory
    #[error("Failed to create parent directory for database: {0}")]
    DirectoryCreationFailed(#[from] std::io::Error),

    /// libsql operation error
    #[error("Database operation failed: {0}")]
    LibsqlError(#[from] libsql::Error),

    /// SQL execution error with context
    #[error("SQL execution failed: {context}")]
    SqlExecutionError { context: String },
}

impl DatabaseError {
    /// Create a connection failed error
    pub fn connection_failed(path: PathBuf, source: libsql::Error) -> Self {
        Self::ConnectionFailed { path, source }
    }

    /// Create an initialization failed error
    pub fn initialization_failed(msg: impl Into<String>) -> Self {
        Self::InitializationFailed(msg.into())
    }

    /// Create an invalid path error
    pub fn invalid_path(path: PathBuf) -> Self {
        Self::InvalidPath { path }
    }

    /// Create a permission denied error
    pub fn permission_denied(path: PathBuf) -> Self {
        Self::PermissionDenied { path }
    }

    /// Create a SQL execution error with context
    pub fn sql_execution(context: impl Into<String>) -> Self {
        Self::SqlExecutionError {
            context: context.into(),
        }
    }
}
