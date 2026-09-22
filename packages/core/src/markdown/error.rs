//! Error type for the transport-agnostic markdown and schema library functions.
//!
//! Replaces the former MCP `MCPError` dependency now that these functions live
//! in core as a normal library (consumed by `nodespace-agent`, `nodespace-daemon`,
//! and benches) rather than behind the deleted MCP JSON-RPC transport.

use std::fmt;

/// Error returned by markdown/schema library functions.
///
/// Consumers only need a `Debug`/`Display` error (the agent surfaces these to the
/// model via `Display`, the daemon logs `?e`), so this intentionally stays small.
///
/// The `Display` text reaches the model verbatim as a tool result, so variant
/// messages should read as repair instructions, not as internal diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownError {
    /// Request parameters were missing or malformed.
    InvalidParams(String),
    /// A referenced node could not be found.
    NotFound(String),
    /// Node creation failed.
    CreationFailed(String),
    /// An unexpected internal error occurred.
    Internal(String),
    /// Something already exists at `id` (currently: a schema id requested by
    /// `create_schema`).
    ///
    /// Structured so a caller that needs to distinguish "this id is taken"
    /// from every other rejection — the methodology installer's collision
    /// re-keying, for one — can match the variant instead of substring-testing
    /// `Display`'s text, which breaks the moment some unrelated rejection
    /// grows the same phrase. `message` carries the full, agent-facing
    /// explanation (e.g. the existing schema's rendered definition); `id` is
    /// the exact, structured signal.
    AlreadyExists { id: String, message: String },
}

impl MarkdownError {
    /// Construct an `InvalidParams` error.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        MarkdownError::InvalidParams(message.into())
    }

    /// Construct an `AlreadyExists` error for the id already taken, with the
    /// full agent-facing explanation as `message`.
    pub fn already_exists(id: impl Into<String>, message: impl Into<String>) -> Self {
        MarkdownError::AlreadyExists {
            id: id.into(),
            message: message.into(),
        }
    }

    /// Construct an `Internal` error.
    pub fn internal_error(message: impl Into<String>) -> Self {
        MarkdownError::Internal(message.into())
    }

    /// Construct a `NotFound` error for the given node id.
    pub fn node_not_found(node_id: &str) -> Self {
        MarkdownError::NotFound(format!("Node not found: {node_id}"))
    }

    /// Construct a `CreationFailed` error.
    pub fn node_creation_failed(message: impl Into<String>) -> Self {
        MarkdownError::CreationFailed(message.into())
    }

    /// The human-readable message for this error.
    pub fn message(&self) -> &str {
        match self {
            MarkdownError::InvalidParams(m)
            | MarkdownError::NotFound(m)
            | MarkdownError::CreationFailed(m)
            | MarkdownError::Internal(m) => m,
            MarkdownError::AlreadyExists { message, .. } => message,
        }
    }
}

impl fmt::Display for MarkdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MarkdownError::InvalidParams(m) => write!(f, "invalid params: {m}"),
            MarkdownError::NotFound(m) => write!(f, "not found: {m}"),
            MarkdownError::CreationFailed(m) => write!(f, "node creation failed: {m}"),
            MarkdownError::Internal(m) => write!(f, "{m}"),
            MarkdownError::AlreadyExists { message, .. } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for MarkdownError {}
