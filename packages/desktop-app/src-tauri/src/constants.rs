//! Shared constants used across the application
//!
//! This module contains constants that are used in multiple places
//! to ensure consistency and avoid duplication.

/// Allowed node types in the system
///
/// Currently supported node types:
/// - "text": Plain text nodes
/// - "header": Markdown headers (h1-h6)
/// - "task": Task/todo nodes with completion status
/// - "date": Date-based nodes for daily notes
/// - "code-block": Code blocks with language selection
/// - "quote-block": Quote block nodes with markdown styling
/// - "ordered-list": Auto-numbered ordered list items
///
/// Used by both Tauri IPC commands and HTTP dev server endpoints
/// to ensure consistent validation.
pub const ALLOWED_NODE_TYPES: &[&str] = &[
    "text",
    "header",
    "task",
    "date",
    "code-block",
    "quote-block",
    "ordered-list",
];
