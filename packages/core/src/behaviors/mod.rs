//! Node Behavior System
//!
//! This module provides the trait-based behavior system for different node types:
//!
//! - `NodeBehavior` trait - Defines type-specific validation and processing
//! - Built-in behaviors (TextNodeBehavior, TaskNodeBehavior, DateNodeBehavior)
//! - `NodeBehaviorRegistry` - Dynamic behavior lookup and registration
//!
//! The behavior system enables extensibility while maintaining type safety
//! and consistent validation across all node operations.

use crate::models::schema::SchemaField;
use crate::models::{Node, SchemaNode, TaskNode, ValidationError as NodeValidationError};
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use thiserror::Error;

/// Lazy-initialized regex for date validation (compiled once)
static DATE_PATTERN: OnceLock<regex::Regex> = OnceLock::new();

/// Returns the compiled date validation regex, initializing it on first use
fn get_date_pattern() -> &'static regex::Regex {
    DATE_PATTERN.get_or_init(|| {
        regex::Regex::new(r"^\d{4}-\d{2}-\d{2}$")
            .expect("Invalid date regex pattern (this is a bug)")
    })
}

/// Errors that can occur during content processing
///
/// These errors are returned by the `NodeBehavior::process_content()` method
/// when content transformation or validation fails.
///
/// # Variant Usage Guidelines
///
/// - **`ProcessingFailed`**: Use for general processing failures that don't fit
///   the other categories. Examples:
///   - External service unavailable (e.g., markdown parser service down)
///   - Resource exhaustion (e.g., content too large to process)
///   - Unexpected processing state or configuration errors
///
/// - **`InvalidFormat`**: Use when the input content format is malformed or
///   cannot be parsed. Examples:
///   - Malformed markdown syntax that cannot be parsed
///   - Invalid JSON in content that should be JSON
///   - Unrecognized or unsupported content encoding
///
/// - **`TransformationError`**: Use when the transformation logic itself fails
///   after successfully parsing the input. Examples:
///   - Sanitization failed to remove unsafe content
///   - Content normalization produced invalid output
///   - Conversion between formats failed (e.g., markdown → HTML)
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::ProcessingError;
///
/// // Invalid format - malformed input
/// let err = ProcessingError::InvalidFormat(
///     "Markdown contains unclosed code fence".to_string()
/// );
///
/// // Transformation error - logic failure
/// let err = ProcessingError::TransformationError(
///     "Failed to sanitize HTML: invalid entity".to_string()
/// );
///
/// // General processing failure
/// let err = ProcessingError::ProcessingFailed(
///     "Content exceeds maximum size limit".to_string()
/// );
/// ```
#[derive(Error, Debug, Clone, PartialEq)]
pub enum ProcessingError {
    /// General processing failure (service unavailable, resource exhaustion, etc.)
    #[error("Content processing failed: {0}")]
    ProcessingFailed(String),

    /// Input content format is malformed or cannot be parsed
    #[error("Invalid content format: {0}")]
    InvalidFormat(String),

    /// Content transformation logic failed after successful parsing
    #[error("Content transformation error: {0}")]
    TransformationError(String),
}

/// Core trait for node type-specific behavior
///
/// This trait defines the contract for all node type behaviors in NodeSpace.
/// Implementations provide type-specific validation, content processing, and
/// capability queries.
///
/// # Thread Safety
///
/// All implementations must be `Send + Sync` to support concurrent access
/// from multiple threads.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, ProcessingError};
/// use nodespace_core::models::{Node, ValidationError};
///
/// struct CustomBehavior;
///
/// impl NodeBehavior for CustomBehavior {
///     fn type_name(&self) -> &'static str {
///         "custom"
///     }
///
///     fn validate(&self, node: &Node) -> Result<(), ValidationError> {
///         if node.content.is_empty() {
///             return Err(ValidationError::MissingField("content".to_string()));
///         }
///         Ok(())
///     }
///
///     fn can_have_children(&self) -> bool {
///         true
///     }
///
///     fn supports_markdown(&self) -> bool {
///         false
///     }
/// }
/// ```
pub trait NodeBehavior: Send + Sync {
    /// Returns the unique type identifier for this node type
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::behaviors::{NodeBehavior, TextNodeBehavior};
    /// let behavior = TextNodeBehavior;
    /// assert_eq!(behavior.type_name(), "text");
    /// ```
    fn type_name(&self) -> &'static str;

    /// Validates node content and properties
    ///
    /// Implementations should check:
    /// - Required fields are present
    /// - Field values are in valid ranges
    /// - Type-specific constraints are met
    ///
    /// # Arguments
    ///
    /// * `node` - The node to validate
    ///
    /// # Errors
    ///
    /// Returns `ValidationError` if validation fails
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::behaviors::{NodeBehavior, TaskNodeBehavior};
    /// # use nodespace_core::models::Node;
    /// # use serde_json::json;
    /// let behavior = TaskNodeBehavior;
    /// let node = Node::new(
    ///     "task".to_string(),
    ///     "Do something".to_string(),
    ///     json!({"status": "open"}),
    /// );
    /// assert!(behavior.validate(&node).is_ok());
    /// ```
    fn validate(&self, node: &Node) -> Result<(), NodeValidationError>;

    /// Returns whether this node type can have children
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::behaviors::{NodeBehavior, TextNodeBehavior, DateNodeBehavior};
    /// let text_behavior = TextNodeBehavior;
    /// assert!(text_behavior.can_have_children());
    ///
    /// let date_behavior = DateNodeBehavior;
    /// assert!(date_behavior.can_have_children());
    /// ```
    fn can_have_children(&self) -> bool;

    /// Returns whether this node type supports markdown formatting
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::behaviors::{NodeBehavior, TextNodeBehavior, TaskNodeBehavior};
    /// let text_behavior = TextNodeBehavior;
    /// assert!(text_behavior.supports_markdown());
    ///
    /// let task_behavior = TaskNodeBehavior;
    /// assert!(!task_behavior.supports_markdown());
    /// ```
    fn supports_markdown(&self) -> bool;

    /// Processes and transforms content before storage
    ///
    /// Default implementation returns content unchanged. Override to provide
    /// type-specific content processing (e.g., markdown parsing, sanitization).
    ///
    /// # Arguments
    ///
    /// * `content` - The raw content to process
    ///
    /// # Errors
    ///
    /// Returns `ProcessingError` if content processing fails
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::behaviors::{NodeBehavior, TextNodeBehavior};
    /// let behavior = TextNodeBehavior;
    /// let processed = behavior.process_content("Hello world").unwrap();
    /// assert_eq!(processed, "Hello world");
    /// ```
    fn process_content(&self, content: &str) -> Result<String, ProcessingError> {
        Ok(content.to_string())
    }

    /// Returns default metadata/properties for new nodes of this type
    ///
    /// # Examples
    ///
    /// ```rust
    /// # use nodespace_core::behaviors::{NodeBehavior, TaskNodeBehavior};
    /// # use serde_json::json;
    /// let behavior = TaskNodeBehavior;
    /// let defaults = behavior.default_metadata();
    /// // Task defaults use nested format: properties.task.status
    /// // Status values use lowercase format (Issue #670)
    /// assert_eq!(defaults["task"]["status"], "open");
    /// ```
    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({})
    }

    /// Returns embeddable text content if this node should be embedded as a root.
    ///
    /// Returns `Some(text)` if:
    /// - Node should be embedded on its own (text nodes, headers with content)
    ///
    /// Returns `None` if:
    /// - Node only contributes to parent embedding (don't embed as standalone)
    /// - Empty/whitespace-only content
    ///
    /// # Examples
    ///
    /// Text nodes: Returns the full content
    /// Headers: Returns the header text (behavior decides if should be root)
    /// Tasks: Returns None (only contribute to parent)
    fn get_embeddable_content(&self, node: &Node) -> Option<String> {
        if node.content.trim().is_empty() {
            None
        } else {
            Some(node.content.clone())
        }
    }

    /// Returns content this node contributes to its parent's embedding.
    ///
    /// Returns `Some(text)` if node has content that should be included when
    /// parent node is embedded (node contributes to parent's embedding).
    ///
    /// Returns `None` if node doesn't contribute to parent.
    ///
    /// # Examples
    ///
    /// Text nodes: Returns the full content (contributes to parent)
    /// Headers: Returns the header text (contributes title to parent)
    /// Tasks: Returns None (don't contribute to parent date nodes)
    /// Dates: Returns None (dates don't nest under other nodes)
    fn get_parent_contribution(&self, node: &Node) -> Option<String> {
        if node.content.trim().is_empty() {
            None
        } else {
            Some(node.content.clone())
        }
    }
}

/// Check if a string contains only whitespace (including Unicode whitespace)
///
/// This function validates that content has at least one non-whitespace character.
/// It handles both ASCII and Unicode whitespace characters including:
/// - Standard ASCII whitespace (space, tab, newline, etc.) - covered by is_whitespace()
/// - Non-breaking spaces (U+00A0) - covered by is_whitespace()
/// - Unicode line/paragraph separators (U+2028, U+2029) - covered by is_whitespace()
/// - Zero-width spaces (U+200B, U+200C, U+200D) - NOT covered by is_whitespace(), checked explicitly
/// - Zero-width no-break space / BOM (U+FEFF) - checked explicitly
///
/// Note: Rust's char::is_whitespace() follows Unicode's "White_Space" property,
/// which intentionally excludes zero-width spaces as they're meant for text shaping,
/// not spacing. However, for content validation, we treat them as empty.
///
/// # Arguments
///
/// * `content` - The string to check
///
/// # Returns
///
/// `true` if the string is empty or contains only whitespace/invisible characters
fn is_empty_or_whitespace(content: &str) -> bool {
    content.chars().all(|c| {
        c.is_whitespace()
            || c == '\u{200B}' // Zero-width space
            || c == '\u{200C}' // Zero-width non-joiner
            || c == '\u{200D}' // Zero-width joiner
            || c == '\u{FEFF}' // Zero-width no-break space (BOM)
    })
}

/// Built-in behavior for text nodes
///
/// Text nodes support markdown formatting and can contain other nodes.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, TextNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = TextNodeBehavior;
/// let node = Node::new(
///     "text".to_string(),
///     "Hello world".to_string(),
///     json!({}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct TextNodeBehavior;

impl NodeBehavior for TextNodeBehavior {
    fn type_name(&self) -> &'static str {
        "text"
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        // Issue #479 Phase 1: Allow blank text nodes
        // Changed behavior: Backend now accepts blank text nodes
        //
        // Architecture Change (Issue #479):
        // - Frontend: Persists blank nodes immediately (with 500ms debounce)
        // - Backend: Accepts blank text nodes (user responsible for managing them)
        // - User Experience: Users can create blank nodes via Enter key, burden is on user to maintain or delete
        //
        // This change:
        // 1. Eliminates ephemeral-during-editing behavior
        // 2. Prevents UNIQUE constraint violations when indenting blank nodes
        // 3. Simplifies frontend persistence logic (Phase 2 will remove deferred update queue)
        //
        // Previous behavior (before Issue #479):
        // - Backend rejected blank nodes with validation error
        // - Frontend had to manage ephemeral nodes until content was added
        // - Caused database constraint issues during indent operations

        // Blank text nodes are now allowed - no validation required
        Ok(())
    }

    fn can_have_children(&self) -> bool {
        true
    }

    fn supports_markdown(&self) -> bool {
        true
    }

    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "markdown_enabled": true,
            "auto_save": true,
            "word_wrap": true
        })
    }
}

/// Built-in behavior for header nodes
///
/// Header nodes represent markdown headers (h1-h6) with content stored including
/// the hash symbols (e.g., "## Hello" for h2).
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, HeaderNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = HeaderNodeBehavior;
/// let node = Node::new(
///     "header".to_string(),
///     "## Hello World".to_string(),
///     json!({"headerLevel": 2}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct HeaderNodeBehavior;

impl NodeBehavior for HeaderNodeBehavior {
    fn type_name(&self) -> &'static str {
        "header"
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        // Issue #484: Allow blank header nodes (e.g., "##" with no content)
        // Similar to text nodes, headers can be created blank and filled in later
        // Frontend manages the UX of blank headers (e.g., showing placeholder text)
        Ok(())
    }

    fn can_have_children(&self) -> bool {
        true
    }

    fn supports_markdown(&self) -> bool {
        true
    }

    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "headerLevel": 1,
            "markdown_enabled": true
        })
    }
}

/// Built-in behavior for task nodes
///
/// Task nodes represent actionable items with status tracking.
///
/// # Valid Status Values (Schema-Defined)
///
/// Status values are defined in the task schema and validated dynamically.
/// All values use lowercase format for consistency across layers (Issue #670).
///
/// Core values (protected, cannot be removed):
/// - "open" - Not started (default)
/// - "in_progress" - Currently being worked on
/// - "done" - Finished
/// - "cancelled" - Cancelled/abandoned
///
/// User-extensible values can be added via schema.
///
/// # Strongly-Typed Validation (Issue #673)
///
/// This behavior supports both generic Node validation (via `validate()`) and
/// strongly-typed TaskNode validation (via `validate_task_node()`). The generic
/// validation internally converts to TaskNode for type-safe validation.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, TaskNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = TaskNodeBehavior;
/// let node = Node::new(
///     "task".to_string(),
///     "Implement NodeBehavior trait".to_string(),
///     json!({"status": "in_progress"}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct TaskNodeBehavior;

impl TaskNodeBehavior {
    /// Validate a strongly-typed TaskNode directly (Issue #673)
    ///
    /// This method provides compile-time type safety by validating TaskNode
    /// fields directly rather than parsing from JSON properties. Use this
    /// method when you already have a TaskNode instance.
    ///
    /// # Arguments
    ///
    /// * `task` - The TaskNode to validate
    ///
    /// # Errors
    ///
    /// Returns `ValidationError` if validation fails. Currently validates:
    /// - Status is a valid enum value (enforced by TaskStatus type)
    /// - Priority is a valid enum value (enforced by TaskPriority type)
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::behaviors::TaskNodeBehavior;
    /// use nodespace_core::models::{TaskNode, TaskStatus, TaskPriority};
    ///
    /// let behavior = TaskNodeBehavior;
    /// let task = TaskNode::builder("Fix bug".to_string())
    ///     .with_status(TaskStatus::InProgress)
    ///     .with_priority(TaskPriority::Medium)
    ///     .build();
    ///
    /// assert!(behavior.validate_task_node(&task).is_ok());
    /// ```
    pub fn validate_task_node(&self, _task: &TaskNode) -> Result<(), NodeValidationError> {
        // Status is already type-safe via TaskStatus enum - no validation needed
        // The Rust type system guarantees it's a valid status value
        //
        // Priority is already type-safe via TaskPriority enum - no validation needed
        // Core values (low, medium, high) are enforced by the enum
        // User-defined values (TaskPriority::User) are allowed by schema extension
        //
        // Note: Empty content is allowed for tasks - users can add description later
        // Note: Due date validation (if present, must be valid DateTime) is enforced by type

        Ok(())
    }
}

impl NodeBehavior for TaskNodeBehavior {
    fn type_name(&self) -> &'static str {
        "task"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Issue #673: Convert to strongly-typed TaskNode and validate
        // This provides type-safe validation with direct field access
        match TaskNode::from_node(node.clone()) {
            Ok(task) => self.validate_task_node(&task),
            Err(e) => {
                // If conversion fails, fall back to basic property validation
                // This handles backward compatibility with old property formats
                tracing::debug!(
                    "TaskNode conversion failed, using fallback validation: {}",
                    e
                );

                // Type-namespaced property validation (Issue #397)
                // Properties are stored under type-specific namespaces: properties.task.*
                // This allows preserving properties when converting between types
                //
                // BACKWARD COMPATIBILITY: Accept both formats during transition:
                // - New format: properties.task.status
                // - Old format: properties.status (deprecated, will be auto-migrated)

                // Try new nested format first, fall back to old flat format
                let task_props = node.properties.get("task").or(Some(&node.properties));

                // If task properties exist, validate their TYPES (not values)
                // VALUE validation (e.g., valid status enum values) is handled by schema system
                if let Some(props) = task_props {
                    // Validate status type (must be string if present)
                    // Schema system validates the actual value against allowed enum values
                    if let Some(status) = props.get("status") {
                        if !status.is_string() && !status.is_null() {
                            return Err(NodeValidationError::InvalidProperties(
                                "Status must be a string".to_string(),
                            ));
                        }
                    }

                    // Validate priority type (must be integer if present)
                    if let Some(priority) = props.get("priority") {
                        if !priority.is_i64() && !priority.is_null() {
                            return Err(NodeValidationError::InvalidProperties(
                                "Priority must be an integer".to_string(),
                            ));
                        }
                    }
                }

                Ok(())
            }
        }
    }

    fn can_have_children(&self) -> bool {
        true // Tasks can have subtasks
    }

    fn supports_markdown(&self) -> bool {
        false // Tasks are usually single-line descriptions
    }

    fn default_metadata(&self) -> serde_json::Value {
        // Uses lowercase canonical values for consistency across all layers (Issue #670)
        serde_json::json!({
            "task": {
                "status": "open",
                "priority": "medium",
                "due_date": null,
                "assignee_id": null
            }
        })
    }

    /// Tasks are not embedded as standalone roots
    ///
    /// Tasks are typically action items under date nodes or projects.
    /// They don't carry semantic content worth embedding independently.
    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    /// Tasks don't contribute to parent embeddings
    ///
    /// Tasks under date nodes shouldn't pollute the date's semantic embedding.
    /// The date node represents "what I worked on" not "my todo list".
    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Built-in behavior for code block nodes
///
/// Code block nodes contain code snippets with language selection.
/// Content includes the markdown fence syntax (e.g., "```javascript\ncode here").
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, CodeBlockNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = CodeBlockNodeBehavior;
/// let node = Node::new(
///     "code-block".to_string(),
///     "```javascript\nconst x = 1;".to_string(),
///     json!({"language": "javascript"}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct CodeBlockNodeBehavior;

impl NodeBehavior for CodeBlockNodeBehavior {
    fn type_name(&self) -> &'static str {
        "code-block"
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        // Issue #484: Allow blank code blocks (e.g., "```language" with no code)
        // Users can create blank code blocks and fill in code later
        // Frontend manages the UX of blank code blocks (e.g., showing placeholder text)
        Ok(())
    }

    fn can_have_children(&self) -> bool {
        false // Code blocks are leaf nodes
    }

    fn supports_markdown(&self) -> bool {
        false // Code blocks display raw text, no markdown formatting
    }

    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "language": "plaintext"
        })
    }
}

/// Built-in behavior for quote block nodes
///
/// Quote block nodes represent block quotes with markdown styling conventions.
/// Content includes the > prefix (e.g., "> Quote text").
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, QuoteBlockNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = QuoteBlockNodeBehavior;
/// let node = Node::new(
///     "quote-block".to_string(),
///     "> Hello world".to_string(),
///     json!({}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct QuoteBlockNodeBehavior;

impl NodeBehavior for QuoteBlockNodeBehavior {
    fn type_name(&self) -> &'static str {
        "quote-block"
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        // Issue #484: Allow blank quote blocks (e.g., ">" with no content)
        // Users can create blank quote blocks and fill in quoted text later
        // Frontend manages the UX of blank quote blocks (e.g., showing placeholder text)
        Ok(())
    }

    fn can_have_children(&self) -> bool {
        true // Quote blocks can have children
    }

    fn supports_markdown(&self) -> bool {
        true // Quote blocks support inline markdown formatting
    }

    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "markdown_enabled": true
        })
    }
}

/// Built-in behavior for ordered list nodes
///
/// Ordered list nodes represent auto-numbered list items with CSS counter-based
/// numbering in the UI. Content includes the "1. " prefix (e.g., "1. First item").
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, OrderedListNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = OrderedListNodeBehavior;
/// let node = Node::new(
///     "ordered-list".to_string(),
///     "1. Hello world".to_string(),
///     json!({}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct OrderedListNodeBehavior;

impl NodeBehavior for OrderedListNodeBehavior {
    fn type_name(&self) -> &'static str {
        "ordered-list"
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        // Issue #484: Allow blank ordered list nodes (consistent with headers, quotes, etc.)
        //
        // ARCHITECTURAL DECISION: Blank ordered lists are semantically valid:
        //
        // 1. The "1. " prefix is STRUCTURAL SYNTAX, not user content
        //    - Similar to how markdown "## " is structural for headers
        //    - The prefix defines the node type and formatting
        //    - Just like blank headers ("##"), blank ordered lists ("1. ") are valid
        //
        // 2. Empty ordered list items are semantically valid
        //    - HTML allows <li></li> (empty list items)
        //    - Markdown allows "1. " as valid syntax
        //    - Users may intentionally create empty list items as placeholders
        //
        // 3. Consistent with frontend UX expectations
        //    - Pressing Enter creates new list item with "1. " prefix
        //    - User expects immediate persistence without requiring content first
        //    - Backend should accept what frontend naturally generates
        //
        // 4. Consistency with other node types (Issue #484)
        //    - Headers allow blank content after "##"
        //    - Quote blocks allow blank content after ">"
        //    - Code blocks allow blank content after "```"
        //    - Ordered lists should allow blank content after "1. "
        //
        // Frontend manages the UX of blank ordered list nodes (e.g., showing placeholder text)
        Ok(())
    }

    fn can_have_children(&self) -> bool {
        false // Ordered lists are leaf nodes
    }

    fn supports_markdown(&self) -> bool {
        true // Ordered lists support inline markdown formatting
    }

    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "markdown_enabled": true
        })
    }
}

/// Built-in behavior for date nodes
///
/// Date nodes use deterministic IDs in YYYY-MM-DD format and serve as
/// containers for daily notes and time-based organization.
///
/// # ID Format
///
/// Date nodes must have IDs matching `YYYY-MM-DD` (e.g., "2025-01-03").
/// Per Issue #670, the content field can be any custom content (no longer
/// required to match the date ID). Date nodes have no spoke table - all
/// data is stored in the hub's content field.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, DateNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = DateNodeBehavior;
/// let node = Node::new_with_id(
///     "2025-01-03".to_string(),
///     "date".to_string(),
///     "Custom Daily Notes".to_string(), // Content can be anything
///     json!({}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct DateNodeBehavior;

impl NodeBehavior for DateNodeBehavior {
    fn type_name(&self) -> &'static str {
        "date"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Validate date ID format (YYYY-MM-DD) using lazy-compiled regex
        if !get_date_pattern().is_match(&node.id) {
            return Err(NodeValidationError::InvalidId(
                "Date nodes must have ID format 'YYYY-MM-DD'".to_string(),
            ));
        }

        // Validate that it's an actual valid date using chrono
        use chrono::NaiveDate;
        NaiveDate::parse_from_str(&node.id, "%Y-%m-%d").map_err(|_| {
            NodeValidationError::InvalidId(format!("Invalid date format: {}", node.id))
        })?;

        // NOTE: Per Issue #670, date nodes can have custom content (not required to match ID).
        // The ID is always in YYYY-MM-DD format, but content can be anything (e.g., "Custom Date Content").
        // Date nodes no longer have a spoke table - all data is stored in the hub's content field.

        Ok(())
    }

    fn can_have_children(&self) -> bool {
        true // Dates can contain events, tasks, etc.
    }

    fn supports_markdown(&self) -> bool {
        false // Dates are simple identifiers
    }

    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "is_holiday": false,
            "timezone": "UTC"
        })
    }

    /// Date nodes are not embedded (they're containers)
    ///
    /// Date nodes are organizational containers, not semantic content.
    /// Their children (text nodes) carry the actual embeddable content.
    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    /// Dates don't nest under other nodes
    ///
    /// Date nodes are always root-level containers, they never contribute
    /// content to a parent's embedding.
    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Schema node behavior
///
/// Schema nodes store entity type definitions using the Pure JSON schema-as-node pattern.
/// By convention, schema nodes have `id = type_name` and `node_type = "schema"`.
///
/// Validation includes:
/// - Non-empty content (schema name)
/// - Properties must be valid JSON object
/// - Field names must be unique (alphanumeric and underscores only)
/// - Enum fields must have at least one value defined (in core_values or user_values)
///
/// # Strongly-Typed Validation
///
/// This behavior supports both generic Node validation (via `validate()`) and
/// strongly-typed SchemaNode validation (via `validate_schema_node()`). The generic
/// validation internally converts to SchemaNode for type-safe validation.
pub struct SchemaNodeBehavior;

/// Validate a single schema field (standalone function for recursive validation)
fn validate_schema_field(field: &SchemaField) -> Result<(), NodeValidationError> {
    // Validate field name characters (alphanumeric and underscores only)
    if !field.name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Err(NodeValidationError::InvalidProperties(format!(
            "Invalid field name '{}': must contain only alphanumeric characters and underscores",
            field.name
        )));
    }

    // Enum fields must have at least one value defined
    if field.field_type == "enum" {
        let has_values = field.core_values.as_ref().is_some_and(|v| !v.is_empty())
            || field.user_values.as_ref().is_some_and(|v| !v.is_empty());

        if !has_values {
            return Err(NodeValidationError::InvalidProperties(format!(
                "Enum field '{}' must have at least one value defined (in core_values or user_values)",
                field.name
            )));
        }
    }

    // Recursively validate nested fields
    if let Some(ref nested_fields) = field.fields {
        for nested_field in nested_fields {
            validate_schema_field(nested_field)?;
        }
    }

    // Recursively validate item fields (for array of objects)
    if let Some(ref item_fields) = field.item_fields {
        for item_field in item_fields {
            validate_schema_field(item_field)?;
        }
    }

    Ok(())
}

impl SchemaNodeBehavior {
    /// Validate a strongly-typed SchemaNode directly
    ///
    /// This method provides compile-time type safety by validating SchemaNode
    /// fields directly rather than parsing from JSON properties. Use this
    /// method when you already have a SchemaNode instance.
    ///
    /// # Arguments
    ///
    /// * `schema` - The SchemaNode to validate
    ///
    /// # Errors
    ///
    /// Returns `ValidationError` if validation fails. Validates:
    /// - Content (schema name) is non-empty
    /// - Schema version is positive
    /// - Field names are unique and valid
    /// - Enum fields have at least one value
    pub fn validate_schema_node(&self, schema: &SchemaNode) -> Result<(), NodeValidationError> {
        // Validate non-empty content (schema name)
        if is_empty_or_whitespace(&schema.content) {
            return Err(NodeValidationError::MissingField(
                "Schema nodes must have content (schema name)".to_string(),
            ));
        }

        // Validate schema version is positive
        if schema.schema_version == 0 {
            return Err(NodeValidationError::InvalidProperties(
                "Schema version must be positive".to_string(),
            ));
        }

        // Validate field name uniqueness
        let field_names: HashSet<_> = schema.fields.iter().map(|f| &f.name).collect();
        if field_names.len() != schema.fields.len() {
            return Err(NodeValidationError::InvalidProperties(
                "Schema contains duplicate field names".to_string(),
            ));
        }

        // Validate each field
        for field in &schema.fields {
            validate_schema_field(field)?;
        }

        Ok(())
    }
}

impl NodeBehavior for SchemaNodeBehavior {
    fn type_name(&self) -> &'static str {
        "schema"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Convert to strongly-typed SchemaNode and validate
        // This provides type-safe validation with direct field access
        match SchemaNode::from_node(node.clone()) {
            Ok(schema) => self.validate_schema_node(&schema),
            Err(e) => {
                // If conversion fails, fall back to basic validation
                tracing::debug!(
                    "SchemaNode conversion failed, using fallback validation: {}",
                    e
                );

                // Basic validation - non-empty content
                if is_empty_or_whitespace(&node.content) {
                    return Err(NodeValidationError::MissingField(
                        "Schema nodes must have content (schema name)".to_string(),
                    ));
                }

                // Properties should be valid JSON object
                if !node.properties.is_object() {
                    return Err(NodeValidationError::InvalidProperties(
                        "Schema properties must be a JSON object".to_string(),
                    ));
                }

                Ok(())
            }
        }
    }

    fn can_have_children(&self) -> bool {
        false // Schemas are metadata nodes, not containers
    }

    fn supports_markdown(&self) -> bool {
        false // Schemas are structured data
    }

    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "is_core": false,
            "version": 1,
            "description": "",
            "fields": []
        })
    }
}

/// Fallback behavior for schema-defined custom types
///
/// This behavior is used for node types that have a schema definition but no
/// explicit `NodeBehavior` implementation. It provides minimal validation
/// suitable for user-defined entity types like "person", "invoice", "customer", etc.
///
/// # Validation Rules
///
/// - Content can be empty (user-defined types may use properties only)
/// - Properties must be a valid JSON object
/// - Schema-level validation is handled separately by `NodeService`
///
/// # Usage
///
/// This behavior is automatically used as a fallback by `NodeBehaviorRegistry`
/// when no specific behavior is registered for a node type. It enables schema-driven
/// extensibility without requiring Rust code for each custom type.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, CustomNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = CustomNodeBehavior::new("person");
/// let node = Node::new(
///     "person".to_string(),
///     "John Doe".to_string(),
///     json!({"email": "john@example.com"}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct CustomNodeBehavior {
    type_name: String,
}

impl CustomNodeBehavior {
    /// Creates a new CustomNodeBehavior for a specific type name
    pub fn new(type_name: &str) -> Self {
        Self {
            type_name: type_name.to_string(),
        }
    }
}

impl NodeBehavior for CustomNodeBehavior {
    fn type_name(&self) -> &'static str {
        // SAFETY: This is a workaround for the 'static lifetime requirement.
        // The type_name is stored in the struct and lives as long as the behavior.
        // We leak the string to get a static reference. This is acceptable because:
        // 1. CustomNodeBehavior instances are long-lived (stored in registry)
        // 2. The number of custom types is bounded by user-defined schemas
        // 3. Memory is reclaimed when the process exits
        Box::leak(self.type_name.clone().into_boxed_str())
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Minimal validation for schema-defined custom types:
        // - Content can be empty (some entity types may use properties only)
        // - Properties must be a valid JSON object
        if !node.properties.is_object() {
            return Err(NodeValidationError::InvalidProperties(
                "Properties must be a JSON object".to_string(),
            ));
        }
        Ok(())
    }

    fn can_have_children(&self) -> bool {
        true // Custom types can have children by default
    }

    fn supports_markdown(&self) -> bool {
        false // Custom types don't support markdown by default
    }

    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({})
    }
}

/// Registry for managing node behaviors
///
/// The registry provides thread-safe storage and retrieval of node behaviors.
/// Built-in behaviors (text, task, date) are registered automatically.
///
/// # Fallback Behavior
///
/// For node types without explicit behavior registration, the registry provides
/// a `CustomNodeBehavior` fallback. This enables schema-defined custom types
/// (like "person", "invoice", "customer") to work without Rust code.
///
/// # Thread Safety
///
/// All behaviors are stored in `Arc` for efficient cloning and thread-safe access
/// during read operations. The registry follows a "register at startup, read at runtime"
/// pattern:
///
/// - **Concurrent reads**: Safe without external synchronization. Wrap in `Arc<NodeBehaviorRegistry>`
///   to share across threads (see test_registry_thread_safety for example).
/// - **Concurrent registration**: Requires external synchronization. Wrap in `Arc<Mutex<NodeBehaviorRegistry>>`
///   if registering behaviors from multiple threads.
///
/// For most applications, behaviors are registered once during initialization and then
/// accessed concurrently during runtime, making external synchronization unnecessary
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::NodeBehaviorRegistry;
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let registry = NodeBehaviorRegistry::new();
///
/// // Validate a node using registered behavior
/// let node = Node::new(
///     "text".to_string(),
///     "Hello".to_string(),
///     json!({}),
/// );
/// assert!(registry.validate_node(&node).is_ok());
///
/// // Get all registered types
/// let types = registry.get_all_types();
/// assert!(types.contains(&"text".to_string()));
/// assert!(types.contains(&"task".to_string()));
/// assert!(types.contains(&"date".to_string()));
///
/// // Custom types also validate (fallback behavior)
/// let custom_node = Node::new(
///     "person".to_string(),
///     "Alice".to_string(),
///     json!({"email": "alice@example.com"}),
/// );
/// assert!(registry.validate_node(&custom_node).is_ok());
/// ```
pub struct NodeBehaviorRegistry {
    behaviors: HashMap<String, Arc<dyn NodeBehavior>>,
}

impl NodeBehaviorRegistry {
    /// Creates a new registry with built-in behaviors registered
    ///
    /// Automatically registers:
    /// - TextNodeBehavior ("text")
    /// - TaskNodeBehavior ("task")
    /// - DateNodeBehavior ("date")
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::behaviors::NodeBehaviorRegistry;
    ///
    /// let registry = NodeBehaviorRegistry::new();
    /// assert!(registry.get("text").is_some());
    /// assert!(registry.get("task").is_some());
    /// assert!(registry.get("date").is_some());
    /// ```
    pub fn new() -> Self {
        let mut registry = Self {
            behaviors: HashMap::new(),
        };

        // Register built-in types
        registry.register(Arc::new(TextNodeBehavior));
        registry.register(Arc::new(HeaderNodeBehavior));
        registry.register(Arc::new(TaskNodeBehavior));
        registry.register(Arc::new(CodeBlockNodeBehavior));
        registry.register(Arc::new(QuoteBlockNodeBehavior));
        registry.register(Arc::new(OrderedListNodeBehavior));
        registry.register(Arc::new(DateNodeBehavior));
        registry.register(Arc::new(SchemaNodeBehavior));

        registry
    }

    /// Registers a new node behavior
    ///
    /// The behavior's `type_name()` is used as the key for registration.
    /// If a behavior with the same type name already exists, it will be replaced.
    ///
    /// # Arguments
    ///
    /// * `behavior` - The behavior to register (wrapped in Arc)
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::behaviors::{NodeBehavior, NodeBehaviorRegistry, TextNodeBehavior};
    /// use std::sync::Arc;
    ///
    /// let mut registry = NodeBehaviorRegistry::new();
    /// registry.register(Arc::new(TextNodeBehavior));
    /// assert!(registry.get("text").is_some());
    /// ```
    pub fn register(&mut self, behavior: Arc<dyn NodeBehavior>) {
        let type_name = behavior.type_name().to_string();
        self.behaviors.insert(type_name, behavior);
    }

    /// Retrieves a behavior by node type
    ///
    /// Returns `None` if no behavior is registered for the given type.
    ///
    /// # Arguments
    ///
    /// * `node_type` - The node type identifier
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::behaviors::NodeBehaviorRegistry;
    ///
    /// let registry = NodeBehaviorRegistry::new();
    /// assert!(registry.get("text").is_some());
    /// assert!(registry.get("unknown").is_none());
    /// ```
    pub fn get(&self, node_type: &str) -> Option<Arc<dyn NodeBehavior>> {
        self.behaviors.get(node_type).cloned()
    }

    /// Returns all registered node type identifiers
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::behaviors::NodeBehaviorRegistry;
    ///
    /// let registry = NodeBehaviorRegistry::new();
    /// let types = registry.get_all_types();
    /// assert!(types.len() >= 3); // At least text, task, date
    /// ```
    pub fn get_all_types(&self) -> Vec<String> {
        self.behaviors.keys().cloned().collect()
    }

    /// Validates a node using its registered behavior
    ///
    /// # Arguments
    ///
    /// * `node` - The node to validate
    ///
    /// # Errors
    ///
    /// For unknown types, uses `CustomNodeBehavior` as a fallback. This enables
    /// schema-defined custom types to work without explicit behavior registration.
    ///
    /// Returns validation errors from the behavior's `validate()` method.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::behaviors::NodeBehaviorRegistry;
    /// use nodespace_core::models::Node;
    /// use serde_json::json;
    ///
    /// let registry = NodeBehaviorRegistry::new();
    ///
    /// let valid_node = Node::new(
    ///     "text".to_string(),
    ///     "Hello".to_string(),
    ///     json!({}),
    /// );
    /// assert!(registry.validate_node(&valid_node).is_ok());
    ///
    /// // Custom types use fallback behavior
    /// let custom_node = Node::new(
    ///     "person".to_string(),
    ///     "Alice".to_string(),
    ///     json!({}),
    /// );
    /// assert!(registry.validate_node(&custom_node).is_ok());
    /// ```
    pub fn validate_node(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Get registered behavior or use fallback for custom types
        let behavior: Arc<dyn NodeBehavior> = self
            .get(&node.node_type)
            .unwrap_or_else(|| Arc::new(CustomNodeBehavior::new(&node.node_type)));

        behavior.validate(node)
    }
}

impl Default for NodeBehaviorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_text_node_behavior_validation() {
        let behavior = TextNodeBehavior;

        // Valid text node
        let valid_node = Node::new("text".to_string(), "Hello world".to_string(), json!({}));
        assert!(behavior.validate(&valid_node).is_ok());

        // Issue #479: Blank text nodes are now allowed (frontend manages persistence)
        let mut empty_node = valid_node.clone();
        empty_node.content = "".to_string();
        assert!(behavior.validate(&empty_node).is_ok());

        // Issue #479: Whitespace-only content is also allowed
        let mut whitespace_node = valid_node.clone();
        whitespace_node.content = "   ".to_string();
        assert!(behavior.validate(&whitespace_node).is_ok());
    }

    #[test]
    fn test_text_node_unicode_whitespace_validation() {
        let behavior = TextNodeBehavior;
        let base_node = Node::new("text".to_string(), "Valid".to_string(), json!({}));

        // Issue #479: All whitespace (including Unicode) is now allowed
        // Backend no longer validates content - frontend manages blank node persistence

        // Zero-width space (U+200B) - now allowed
        let mut node = base_node.clone();
        node.content = "\u{200B}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Zero-width space should be allowed per Issue #479"
        );

        // Zero-width non-joiner (U+200C) - now allowed
        node.content = "\u{200C}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Zero-width non-joiner should be allowed per Issue #479"
        );

        // Zero-width joiner (U+200D) - now allowed
        node.content = "\u{200D}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Zero-width joiner should be allowed per Issue #479"
        );

        // Non-breaking space (U+00A0) - now allowed
        node.content = "\u{00A0}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Non-breaking space should be allowed per Issue #479"
        );

        // Line separator (U+2028) - now allowed
        node.content = "\u{2028}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Line separator should be allowed per Issue #479"
        );

        // Paragraph separator (U+2029) - now allowed
        node.content = "\u{2029}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Paragraph separator should be allowed per Issue #479"
        );

        // Mixed Unicode whitespace - now allowed
        node.content = "\u{200B}\u{00A0}\u{2028}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Mixed Unicode whitespace should be allowed per Issue #479"
        );

        // Valid: Actual content with Unicode whitespace mixed in - should be accepted
        node.content = "Hello\u{00A0}World".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Content with Unicode whitespace inside should be accepted"
        );

        // Valid: Emoji content - should be accepted
        node.content = "👍".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Emoji content should be accepted"
        );
    }

    #[test]
    fn test_text_node_behavior_capabilities() {
        let behavior = TextNodeBehavior;

        assert_eq!(behavior.type_name(), "text");
        assert!(behavior.can_have_children());
        assert!(behavior.supports_markdown());
    }

    #[test]
    fn test_text_node_default_metadata() {
        let behavior = TextNodeBehavior;
        let metadata = behavior.default_metadata();

        assert_eq!(metadata["markdown_enabled"], true);
        assert_eq!(metadata["auto_save"], true);
        assert_eq!(metadata["word_wrap"], true);
    }

    #[test]
    fn test_header_node_behavior_validation() {
        let behavior = HeaderNodeBehavior;

        // Valid header with content
        let valid_node = Node::new(
            "header".to_string(),
            "## Hello World".to_string(),
            json!({"headerLevel": 2}),
        );
        assert!(behavior.validate(&valid_node).is_ok());

        // Issue #484: Blank headers are now allowed (e.g., "##" with no content)
        let mut blank_node = valid_node.clone();
        blank_node.content = "".to_string();
        assert!(
            behavior.validate(&blank_node).is_ok(),
            "Blank header nodes should be allowed per Issue #484"
        );

        // Issue #484: Whitespace-only headers are allowed
        let mut whitespace_node = valid_node.clone();
        whitespace_node.content = "   ".to_string();
        assert!(
            behavior.validate(&whitespace_node).is_ok(),
            "Whitespace-only header nodes should be allowed per Issue #484"
        );
    }

    #[test]
    fn test_code_block_behavior_validation() {
        let behavior = CodeBlockNodeBehavior;

        // Valid code block with content
        let valid_node = Node::new(
            "code-block".to_string(),
            "```javascript\nconst x = 1;".to_string(),
            json!({"language": "javascript"}),
        );
        assert!(behavior.validate(&valid_node).is_ok());

        // Issue #484: Blank code blocks are now allowed
        let mut blank_node = valid_node.clone();
        blank_node.content = "".to_string();
        assert!(
            behavior.validate(&blank_node).is_ok(),
            "Blank code blocks should be allowed per Issue #484"
        );

        // Issue #484: Whitespace-only code blocks are allowed
        let mut whitespace_node = valid_node.clone();
        whitespace_node.content = "   ".to_string();
        assert!(
            behavior.validate(&whitespace_node).is_ok(),
            "Whitespace-only code blocks should be allowed per Issue #484"
        );
    }

    #[test]
    fn test_quote_block_behavior_validation() {
        let behavior = QuoteBlockNodeBehavior;

        // Valid quote block with content
        let valid_node = Node::new(
            "quote-block".to_string(),
            "> Hello world".to_string(),
            json!({}),
        );
        assert!(behavior.validate(&valid_node).is_ok());

        // Issue #484: Blank quote blocks are now allowed (e.g., ">" with no content)
        let mut blank_node = valid_node.clone();
        blank_node.content = "".to_string();
        assert!(
            behavior.validate(&blank_node).is_ok(),
            "Blank quote blocks should be allowed per Issue #484"
        );

        // Issue #484: Quote with just prefix and whitespace
        let mut prefix_only_node = valid_node.clone();
        prefix_only_node.content = ">".to_string();
        assert!(
            behavior.validate(&prefix_only_node).is_ok(),
            "Quote blocks with just '>' should be allowed per Issue #484"
        );

        // Issue #484: Quote with prefix and space
        let mut prefix_space_node = valid_node.clone();
        prefix_space_node.content = "> ".to_string();
        assert!(
            behavior.validate(&prefix_space_node).is_ok(),
            "Quote blocks with just '> ' should be allowed per Issue #484"
        );
    }

    #[test]
    fn test_ordered_list_behavior_validation() {
        let behavior = OrderedListNodeBehavior;

        // Valid ordered list with content
        let valid_node = Node::new(
            "ordered-list".to_string(),
            "1. Hello world".to_string(),
            json!({}),
        );
        assert!(behavior.validate(&valid_node).is_ok());

        // Issue #484: Blank ordered lists are now allowed (consistent with headers, quotes, etc.)
        let mut blank_node = valid_node.clone();
        blank_node.content = "".to_string();
        assert!(
            behavior.validate(&blank_node).is_ok(),
            "Blank ordered list nodes should be allowed per Issue #484"
        );

        // Issue #484: Ordered list with just prefix
        let mut prefix_only_node = valid_node.clone();
        prefix_only_node.content = "1. ".to_string();
        assert!(
            behavior.validate(&prefix_only_node).is_ok(),
            "Ordered lists with just '1. ' should be allowed per Issue #484"
        );

        // Issue #484: Whitespace-only ordered lists are allowed
        let mut whitespace_node = valid_node.clone();
        whitespace_node.content = "   ".to_string();
        assert!(
            behavior.validate(&whitespace_node).is_ok(),
            "Whitespace-only ordered list nodes should be allowed per Issue #484"
        );
    }

    #[test]
    fn test_task_node_behavior_validation() {
        let behavior = TaskNodeBehavior;

        // Valid task with status (old flat format - backward compatibility)
        // Status values use lowercase format (Issue #670)
        let valid_node_old_format = Node::new(
            "task".to_string(),
            "Implement feature".to_string(),
            json!({"status": "in_progress"}),
        );
        assert!(behavior.validate(&valid_node_old_format).is_ok());

        // Valid task with status (new nested format - Issue #397)
        let valid_node_new_format = Node::new(
            "task".to_string(),
            "Implement feature".to_string(),
            json!({"task": {"status": "in_progress"}}),
        );
        assert!(behavior.validate(&valid_node_new_format).is_ok());

        // Valid task with all fields (new nested format)
        // Priority can be string ("high") or integer (2) - both supported
        let complete_node = Node::new(
            "task".to_string(),
            "Complete task".to_string(),
            json!({
                "task": {
                    "status": "done",
                    "priority": "high",
                    "due_date": "2025-01-10"
                }
            }),
        );
        assert!(behavior.validate(&complete_node).is_ok());

        // Valid task with legacy integer priority (converted to string enum)
        // Legacy integer format: 1 = high, 2 = medium, 3-4 = low
        let integer_priority_node = Node::new(
            "task".to_string(),
            "Task with legacy integer priority".to_string(),
            json!({"task": {"status": "open", "priority": 2}}),
        );
        assert!(behavior.validate(&integer_priority_node).is_ok());

        // Valid: empty content (allowed for tasks - users can add description later)
        let mut empty_content_node = valid_node_new_format.clone();
        empty_content_node.content = String::new();
        assert!(behavior.validate(&empty_content_node).is_ok());

        // NOTE: Status value validation (e.g., "open" vs custom) is handled by TaskStatus enum.
        // Unknown status values become TaskStatus::User(value) for schema extensibility.

        // Invalid status type (number instead of string) falls back gracefully
        let bad_status_type = Node::new(
            "task".to_string(),
            "Task".to_string(),
            json!({"task": {"status": 123}}), // number instead of string
        );
        // Note: TaskNode::from_node() ignores invalid status and uses default (Open)
        // This is graceful degradation - it passes validation
        assert!(behavior.validate(&bad_status_type).is_ok());

        // Priority is now a string enum (low, medium, high) with user-extensibility
        // Legacy integers are converted: 1=high, 2=medium, 3+=low
        // All values pass validation - unknown values become TaskPriority::User(value)
        let priority_low = Node::new(
            "task".to_string(),
            "Task".to_string(),
            json!({"task": {"priority": "low"}}),
        );
        assert!(behavior.validate(&priority_low).is_ok());

        let priority_medium = Node::new(
            "task".to_string(),
            "Task".to_string(),
            json!({"task": {"priority": "medium"}}),
        );
        assert!(behavior.validate(&priority_medium).is_ok());

        let priority_high = Node::new(
            "task".to_string(),
            "Task".to_string(),
            json!({"task": {"priority": "high"}}),
        );
        assert!(behavior.validate(&priority_high).is_ok());

        // User-defined priority values are allowed (schema extensibility)
        let priority_custom = Node::new(
            "task".to_string(),
            "Task".to_string(),
            json!({"task": {"priority": "critical"}}),
        );
        assert!(
            behavior.validate(&priority_custom).is_ok(),
            "User-defined priority values should be allowed for schema extensibility"
        );
    }

    #[test]
    fn test_task_node_behavior_capabilities() {
        let behavior = TaskNodeBehavior;

        assert_eq!(behavior.type_name(), "task");
        assert!(behavior.can_have_children());
        assert!(!behavior.supports_markdown());
    }

    #[test]
    fn test_task_node_default_metadata() {
        let behavior = TaskNodeBehavior;
        let metadata = behavior.default_metadata();

        // Properties are now nested under "task" namespace (Issue #397)
        // Status/priority values use lowercase format (Issue #670)
        assert_eq!(metadata["task"]["status"], "open");
        assert_eq!(metadata["task"]["priority"], "medium");
        assert!(metadata["task"]["due_date"].is_null());
        assert!(metadata["task"]["assignee_id"].is_null());
    }

    #[test]
    fn test_type_conversion_preserves_properties() {
        // Core value proposition of Issue #397: Properties should be preserved
        // when converting between node types (e.g., task → text → task)

        let behavior = TaskNodeBehavior;

        // Create a task node with properties in the new nested format
        // Status/priority values use lowercase format (Issue #670)
        let mut task_node = Node::new(
            "task".to_string(),
            "Important task".to_string(),
            json!({
                "task": {
                    "status": "in_progress",
                    "priority": "high",
                    "due_date": "2025-01-15"
                }
            }),
        );

        // Verify initial validation passes
        assert!(behavior.validate(&task_node).is_ok());
        assert_eq!(task_node.properties["task"]["status"], "in_progress");
        assert_eq!(task_node.properties["task"]["priority"], "high");

        // Convert to text node (simulate type conversion)
        task_node.node_type = "text".to_string();

        // Task properties should still exist in the properties JSON
        // (even though it's no longer a task node)
        assert!(task_node.properties["task"].is_object());
        assert_eq!(task_node.properties["task"]["status"], "in_progress");
        assert_eq!(task_node.properties["task"]["priority"], "high");
        assert_eq!(task_node.properties["task"]["due_date"], "2025-01-15");

        // Convert back to task node
        task_node.node_type = "task".to_string();

        // Properties should still be there and validate correctly
        assert!(behavior.validate(&task_node).is_ok());
        assert_eq!(task_node.properties["task"]["status"], "in_progress");
        assert_eq!(task_node.properties["task"]["priority"], "high");
        assert_eq!(task_node.properties["task"]["due_date"], "2025-01-15");

        // This demonstrates the key benefit: properties survive type conversions
        // without data loss, enabling flexible node type changes in the UI
    }

    #[test]
    fn test_date_node_behavior_validation() {
        let behavior = DateNodeBehavior;

        // Valid date node
        let valid_node = Node::new_with_id(
            "2025-01-03".to_string(),
            "date".to_string(),
            "2025-01-03".to_string(),
            json!({}),
        );
        assert!(behavior.validate(&valid_node).is_ok());

        // Invalid: bad ID format
        let mut invalid_id = valid_node.clone();
        invalid_id.id = "2025-1-3".to_string();
        assert!(behavior.validate(&invalid_id).is_err());

        // Invalid: not a real date
        let mut invalid_date = valid_node.clone();
        invalid_date.id = "2025-13-45".to_string();
        assert!(behavior.validate(&invalid_date).is_err());

        // Valid: Per Issue #670, content can be different from ID
        let mut custom_content = valid_node.clone();
        custom_content.content = "Custom Daily Notes".to_string();
        assert!(behavior.validate(&custom_content).is_ok());
    }

    #[test]
    fn test_date_node_behavior_capabilities() {
        let behavior = DateNodeBehavior;

        assert_eq!(behavior.type_name(), "date");
        assert!(behavior.can_have_children());
        assert!(!behavior.supports_markdown());
    }

    #[test]
    fn test_date_node_default_metadata() {
        let behavior = DateNodeBehavior;
        let metadata = behavior.default_metadata();

        assert_eq!(metadata["is_holiday"], false);
        assert_eq!(metadata["timezone"], "UTC");
    }

    #[test]
    fn test_registry_new() {
        let registry = NodeBehaviorRegistry::new();

        // Should have built-in behaviors
        assert!(registry.get("text").is_some());
        assert!(registry.get("task").is_some());
        assert!(registry.get("date").is_some());
    }

    #[test]
    fn test_registry_register_and_get() {
        let mut registry = NodeBehaviorRegistry::new();

        // Register a new behavior
        registry.register(Arc::new(TextNodeBehavior));

        // Should be able to retrieve it
        let behavior = registry.get("text");
        assert!(behavior.is_some());
        assert_eq!(behavior.unwrap().type_name(), "text");

        // Unknown type should return None
        assert!(registry.get("unknown").is_none());
    }

    #[test]
    fn test_registry_get_all_types() {
        let registry = NodeBehaviorRegistry::new();
        let types = registry.get_all_types();

        assert!(types.contains(&"text".to_string()));
        assert!(types.contains(&"header".to_string()));
        assert!(types.contains(&"task".to_string()));
        assert!(types.contains(&"code-block".to_string()));
        assert!(types.contains(&"quote-block".to_string()));
        assert!(types.contains(&"ordered-list".to_string()));
        assert!(types.contains(&"date".to_string()));
        assert!(types.contains(&"schema".to_string()));
        assert_eq!(types.len(), 8);
    }

    #[test]
    fn test_registry_validate_node() {
        let registry = NodeBehaviorRegistry::new();

        // Valid text node
        let text_node = Node::new("text".to_string(), "Hello".to_string(), json!({}));
        assert!(registry.validate_node(&text_node).is_ok());

        // Valid task node (status uses lowercase format per Issue #670)
        let task_node = Node::new(
            "task".to_string(),
            "Do something".to_string(),
            json!({"status": "open"}),
        );
        assert!(registry.validate_node(&task_node).is_ok());

        // Unknown node type now uses CustomNodeBehavior fallback and passes basic validation
        let unknown_node = Node::new("unknown".to_string(), "Content".to_string(), json!({}));
        let result = registry.validate_node(&unknown_node);
        assert!(
            result.is_ok(),
            "Unknown node types should use CustomNodeBehavior fallback"
        );

        // But invalid properties (non-object) still fail
        let mut bad_properties_node =
            Node::new("unknown".to_string(), "Content".to_string(), json!({}));
        bad_properties_node.properties = serde_json::json!("not an object");
        let result = registry.validate_node(&bad_properties_node);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(NodeValidationError::InvalidProperties(_))
        ));
    }

    #[test]
    fn test_registry_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let registry = Arc::new(NodeBehaviorRegistry::new());
        let mut handles = vec![];

        // Spawn multiple threads accessing registry
        for _ in 0..10 {
            let registry_clone = Arc::clone(&registry);
            let handle = thread::spawn(move || {
                let behavior = registry_clone.get("text");
                assert!(behavior.is_some());

                let node = Node::new("text".to_string(), "Thread test".to_string(), json!({}));
                assert!(registry_clone.validate_node(&node).is_ok());
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn test_processing_error_types() {
        let err1 = ProcessingError::ProcessingFailed("test".to_string());
        let err2 = ProcessingError::InvalidFormat("format".to_string());
        let err3 = ProcessingError::TransformationError("transform".to_string());

        assert_eq!(format!("{}", err1), "Content processing failed: test");
        assert_eq!(format!("{}", err2), "Invalid content format: format");
        assert_eq!(
            format!("{}", err3),
            "Content transformation error: transform"
        );
    }

    #[test]
    fn test_default_process_content() {
        let behavior = TextNodeBehavior;
        let content = "Test content";
        let result = behavior.process_content(content).unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn test_behavior_trait_object() {
        let behavior: Arc<dyn NodeBehavior> = Arc::new(TextNodeBehavior);

        assert_eq!(behavior.type_name(), "text");
        assert!(behavior.can_have_children());
        assert!(behavior.supports_markdown());

        let node = Node::new("text".to_string(), "Test".to_string(), json!({}));
        assert!(behavior.validate(&node).is_ok());
    }

    // =========================================================================
    // Two-Level Embeddability Tests (Issue #573)
    // =========================================================================

    #[test]
    fn test_text_node_embeddable_content() {
        let behavior = TextNodeBehavior;

        // Text node with content should be embeddable
        let node = Node::new("text".to_string(), "Hello world".to_string(), json!({}));
        let content = behavior.get_embeddable_content(&node);
        assert!(content.is_some());
        assert_eq!(content.unwrap(), "Hello world");

        // Text node should contribute to parent
        let contribution = behavior.get_parent_contribution(&node);
        assert!(contribution.is_some());
        assert_eq!(contribution.unwrap(), "Hello world");
    }

    #[test]
    fn test_text_node_empty_not_embeddable() {
        let behavior = TextNodeBehavior;

        // Empty text node should NOT be embeddable
        let empty_node = Node::new("text".to_string(), "".to_string(), json!({}));
        assert!(behavior.get_embeddable_content(&empty_node).is_none());
        assert!(behavior.get_parent_contribution(&empty_node).is_none());

        // Whitespace-only text node should NOT be embeddable
        let whitespace_node = Node::new("text".to_string(), "   ".to_string(), json!({}));
        assert!(behavior.get_embeddable_content(&whitespace_node).is_none());
        assert!(behavior.get_parent_contribution(&whitespace_node).is_none());
    }

    #[test]
    fn test_header_node_embeddable_content() {
        let behavior = HeaderNodeBehavior;

        // Header node with content should be embeddable
        let node = Node::new(
            "header".to_string(),
            "## Section Title".to_string(),
            json!({"headerLevel": 2}),
        );
        let content = behavior.get_embeddable_content(&node);
        assert!(content.is_some());
        assert_eq!(content.unwrap(), "## Section Title");

        // Header should contribute to parent
        let contribution = behavior.get_parent_contribution(&node);
        assert!(contribution.is_some());
    }

    #[test]
    fn test_task_node_not_embeddable() {
        let behavior = TaskNodeBehavior;

        // Task node should NOT be embeddable as root
        // Status uses lowercase format (Issue #670)
        let node = Node::new(
            "task".to_string(),
            "Buy groceries".to_string(),
            json!({"status": "open"}),
        );
        assert!(
            behavior.get_embeddable_content(&node).is_none(),
            "Task nodes should not be embeddable as roots"
        );

        // Task should NOT contribute to parent
        assert!(
            behavior.get_parent_contribution(&node).is_none(),
            "Task nodes should not contribute to parent embeddings"
        );
    }

    #[test]
    fn test_date_node_not_embeddable() {
        let behavior = DateNodeBehavior;

        // Date node should NOT be embeddable as root
        let node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        assert!(
            behavior.get_embeddable_content(&node).is_none(),
            "Date nodes should not be embeddable (containers only)"
        );

        // Date should NOT contribute to parent
        assert!(
            behavior.get_parent_contribution(&node).is_none(),
            "Date nodes should not contribute to parent embeddings"
        );
    }

    #[test]
    fn test_code_block_uses_default_embeddability() {
        let behavior = CodeBlockNodeBehavior;

        // Code block should use default implementation (embeddable if content exists)
        let node = Node::new(
            "code-block".to_string(),
            "```rust\nfn main() {}".to_string(),
            json!({"language": "rust"}),
        );

        // Uses default implementation - embeddable if non-empty content
        let content = behavior.get_embeddable_content(&node);
        assert!(content.is_some());
        assert_eq!(content.unwrap(), "```rust\nfn main() {}");

        // Contributes to parent
        assert!(behavior.get_parent_contribution(&node).is_some());
    }

    #[test]
    fn test_quote_block_uses_default_embeddability() {
        let behavior = QuoteBlockNodeBehavior;

        // Quote block should use default implementation
        let node = Node::new(
            "quote-block".to_string(),
            "> Some quote".to_string(),
            json!({}),
        );

        let content = behavior.get_embeddable_content(&node);
        assert!(content.is_some());
        assert_eq!(content.unwrap(), "> Some quote");
    }

    #[test]
    fn test_ordered_list_uses_default_embeddability() {
        let behavior = OrderedListNodeBehavior;

        // Ordered list should use default implementation
        let node = Node::new(
            "ordered-list".to_string(),
            "1. First item".to_string(),
            json!({}),
        );

        let content = behavior.get_embeddable_content(&node);
        assert!(content.is_some());
        assert_eq!(content.unwrap(), "1. First item");
    }

    #[test]
    fn test_schema_node_embeddability() {
        let behavior = SchemaNodeBehavior;

        // Schema nodes have content (the schema name)
        let node = Node::new(
            "schema".to_string(),
            "task".to_string(),
            json!({"is_core": true, "fields": []}),
        );

        // Default implementation - embeddable if content exists
        let content = behavior.get_embeddable_content(&node);
        assert!(content.is_some());
    }

    #[test]
    fn test_registry_embeddability_lookup() {
        let registry = NodeBehaviorRegistry::new();

        // Text node - embeddable
        let text_node = Node::new("text".to_string(), "Content".to_string(), json!({}));
        let text_behavior = registry.get("text").unwrap();
        assert!(text_behavior.get_embeddable_content(&text_node).is_some());

        // Task node - not embeddable (status uses lowercase format per Issue #670)
        let task_node = Node::new(
            "task".to_string(),
            "Task content".to_string(),
            json!({"status": "open"}),
        );
        let task_behavior = registry.get("task").unwrap();
        assert!(task_behavior.get_embeddable_content(&task_node).is_none());

        // Date node - not embeddable
        let date_node = Node::new_with_id(
            "2025-01-15".to_string(),
            "date".to_string(),
            "2025-01-15".to_string(),
            json!({}),
        );
        let date_behavior = registry.get("date").unwrap();
        assert!(date_behavior.get_embeddable_content(&date_node).is_none());
    }

    // =========================================================================
    // Strongly-Typed Validation Tests
    // =========================================================================

    #[test]
    fn test_task_node_behavior_validate_task_node() {
        use crate::models::{TaskNode, TaskPriority, TaskStatus};

        let behavior = TaskNodeBehavior;

        // Valid task with all defaults
        let task = TaskNode::builder("Write tests".to_string()).build();
        assert!(behavior.validate_task_node(&task).is_ok());

        // Valid task with specific status
        let task_in_progress = TaskNode::builder("Fix bug".to_string())
            .with_status(TaskStatus::InProgress)
            .build();
        assert!(behavior.validate_task_node(&task_in_progress).is_ok());

        // Valid task with priority (now a string enum)
        let task_with_priority = TaskNode::builder("High priority task".to_string())
            .with_priority(TaskPriority::High)
            .build();
        assert!(behavior.validate_task_node(&task_with_priority).is_ok());

        // Valid task with low priority
        let task_low_priority = TaskNode::builder("Low priority".to_string())
            .with_priority(TaskPriority::Low)
            .build();
        assert!(behavior.validate_task_node(&task_low_priority).is_ok());

        // User-defined priority values are allowed (schema extensibility)
        let task_custom_priority = TaskNode::builder("Custom priority".to_string())
            .with_priority(TaskPriority::User("critical".to_string()))
            .build();
        assert!(
            behavior.validate_task_node(&task_custom_priority).is_ok(),
            "User-defined priority values should be allowed"
        );

        // Valid: empty content (allowed for tasks)
        let empty_task = TaskNode::builder("".to_string()).build();
        assert!(behavior.validate_task_node(&empty_task).is_ok());
    }

    #[test]
    fn test_schema_node_behavior_validate_schema_node() {
        use crate::models::SchemaNode;

        let behavior = SchemaNodeBehavior;

        // Create a valid schema node via from_node
        let valid_node = Node::new(
            "schema".to_string(),
            "Task".to_string(),
            json!({
                "isCore": true,
                "version": 1,
                "description": "Task schema",
                "fields": []
            }),
        );
        let schema = SchemaNode::from_node(valid_node).unwrap();
        assert!(behavior.validate_schema_node(&schema).is_ok());

        // Invalid: empty content
        let empty_content_node = Node::new(
            "schema".to_string(),
            "".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "description": "",
                "fields": []
            }),
        );
        let empty_schema = SchemaNode::from_node(empty_content_node).unwrap();
        assert!(
            behavior.validate_schema_node(&empty_schema).is_err(),
            "Schema with empty content should be rejected"
        );

        // Invalid: whitespace-only content
        let whitespace_node = Node::new(
            "schema".to_string(),
            "   ".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "description": "",
                "fields": []
            }),
        );
        let whitespace_schema = SchemaNode::from_node(whitespace_node).unwrap();
        assert!(
            behavior.validate_schema_node(&whitespace_schema).is_err(),
            "Schema with whitespace-only content should be rejected"
        );
    }

    // =========================================================================
    // SchemaNodeBehavior Validation Tests
    // =========================================================================

    #[test]
    fn test_schema_node_validates_field_uniqueness() {
        let behavior = SchemaNodeBehavior;

        // Schema with duplicate field names should fail
        let duplicate_fields_node = Node::new(
            "schema".to_string(),
            "custom_type".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [
                    {
                        "name": "field1",
                        "type": "text",
                        "protection": "user",
                        "indexed": false
                    },
                    {
                        "name": "field1",
                        "type": "number",
                        "protection": "user",
                        "indexed": false
                    }
                ]
            }),
        );

        let result = behavior.validate(&duplicate_fields_node);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(NodeValidationError::InvalidProperties(ref msg))
                if msg.contains("duplicate field names")
        ));
    }

    #[test]
    fn test_schema_node_enum_requires_values() {
        let behavior = SchemaNodeBehavior;

        // Enum field without any values should fail
        let enum_no_values_node = Node::new(
            "schema".to_string(),
            "custom_type".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [
                    {
                        "name": "status",
                        "type": "enum",
                        "protection": "user",
                        "indexed": false
                    }
                ]
            }),
        );

        let result = behavior.validate(&enum_no_values_node);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(NodeValidationError::InvalidProperties(ref msg))
                if msg.contains("Enum field") && msg.contains("must have at least one value")
        ));

        // Enum with core_values should pass
        let enum_core_values_node = Node::new(
            "schema".to_string(),
            "task".to_string(),
            json!({
                "isCore": true,
                "version": 1,
                "fields": [
                    {
                        "name": "status",
                        "type": "enum",
                        "protection": "core",
                        "coreValues": [
                            { "value": "open", "label": "Open" },
                            { "value": "in_progress", "label": "In Progress" },
                            { "value": "done", "label": "Done" }
                        ],
                        "indexed": false
                    }
                ]
            }),
        );
        assert!(behavior.validate(&enum_core_values_node).is_ok());

        // Enum with user_values should pass
        let enum_user_values_node = Node::new(
            "schema".to_string(),
            "custom_type".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [
                    {
                        "name": "priority",
                        "type": "enum",
                        "protection": "user",
                        "userValues": [
                            { "value": "low", "label": "Low" },
                            { "value": "medium", "label": "Medium" },
                            { "value": "high", "label": "High" }
                        ],
                        "indexed": false
                    }
                ]
            }),
        );
        assert!(behavior.validate(&enum_user_values_node).is_ok());

        // Enum with empty arrays should fail
        let enum_empty_arrays_node = Node::new(
            "schema".to_string(),
            "custom_type".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [
                    {
                        "name": "status",
                        "type": "enum",
                        "protection": "user",
                        "coreValues": [],
                        "userValues": [],
                        "indexed": false
                    }
                ]
            }),
        );
        let result = behavior.validate(&enum_empty_arrays_node);
        assert!(result.is_err());
    }

    #[test]
    fn test_schema_node_valid_schema_passes() {
        let behavior = SchemaNodeBehavior;

        // Comprehensive valid schema with multiple field types
        let valid_schema_node = Node::new(
            "schema".to_string(),
            "project".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "description": "Project management schema",
                "fields": [
                    {
                        "name": "name",
                        "type": "text",
                        "protection": "user",
                        "required": true,
                        "indexed": false
                    },
                    {
                        "name": "status",
                        "type": "enum",
                        "protection": "user",
                        "coreValues": [
                            { "value": "active", "label": "Active" },
                            { "value": "archived", "label": "Archived" }
                        ],
                        "userValues": [
                            { "value": "on_hold", "label": "On Hold" }
                        ],
                        "indexed": false
                    },
                    {
                        "name": "budget",
                        "type": "number",
                        "protection": "user",
                        "indexed": false
                    },
                    {
                        "name": "department",
                        "type": "text",
                        "protection": "user",
                        "indexed": false
                    },
                    {
                        "name": "external_id",
                        "type": "text",
                        "protection": "system",
                        "indexed": false
                    }
                ]
            }),
        );

        let result = behavior.validate(&valid_schema_node);
        assert!(
            result.is_ok(),
            "Valid schema should pass validation: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_schema_node_nested_field_validation() {
        let behavior = SchemaNodeBehavior;

        // Valid nested fields
        let valid_nested_node = Node::new(
            "schema".to_string(),
            "custom_type".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [
                    {
                        "name": "metadata",
                        "type": "object",
                        "protection": "user",
                        "indexed": false,
                        "fields": [
                            {
                                "name": "author",
                                "type": "text",
                                "protection": "user",
                                "indexed": false
                            },
                            {
                                "name": "created_at",
                                "type": "date",
                                "protection": "user",
                                "indexed": false
                            }
                        ]
                    }
                ]
            }),
        );
        assert!(behavior.validate(&valid_nested_node).is_ok());

        // Valid item_fields (array of objects)
        let valid_item_fields_node = Node::new(
            "schema".to_string(),
            "custom_type".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [
                    {
                        "name": "tags",
                        "type": "array",
                        "protection": "user",
                        "indexed": false,
                        "itemType": "object",
                        "itemFields": [
                            {
                                "name": "label",
                                "type": "text",
                                "protection": "user",
                                "indexed": false
                            },
                            {
                                "name": "color",
                                "type": "text",
                                "protection": "user",
                                "indexed": false
                            }
                        ]
                    }
                ]
            }),
        );
        assert!(behavior.validate(&valid_item_fields_node).is_ok());

        // Nested enum validation - enum in nested field must have values
        let nested_enum_no_values_node = Node::new(
            "schema".to_string(),
            "custom_type".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [
                    {
                        "name": "config",
                        "type": "object",
                        "protection": "user",
                        "indexed": false,
                        "fields": [
                            {
                                "name": "mode",
                                "type": "enum",
                                "protection": "user",
                                "indexed": false
                            }
                        ]
                    }
                ]
            }),
        );

        let result = behavior.validate(&nested_enum_no_values_node);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(NodeValidationError::InvalidProperties(ref msg))
                if msg.contains("Enum field") && msg.contains("must have at least one value")
        ));
    }

    // =========================================================================
    // TaskNode from_node Format Tests
    // =========================================================================

    #[test]
    fn test_task_node_from_node_nested_format() {
        use crate::models::{TaskNode, TaskPriority};

        // Test that TaskNode::from_node handles nested format (Issue #397)
        // Priority now uses string enum format
        let nested_node = Node::new(
            "task".to_string(),
            "Nested format task".to_string(),
            json!({
                "task": {
                    "status": "in_progress",
                    "priority": "medium"
                }
            }),
        );

        let task = TaskNode::from_node(nested_node).unwrap();
        assert_eq!(task.content, "Nested format task");
        assert_eq!(task.status.as_str(), "in_progress");
        assert_eq!(task.priority, Some(TaskPriority::Medium));
    }

    #[test]
    fn test_task_node_from_node_flat_format() {
        use crate::models::{TaskNode, TaskPriority};

        // Test flat format (old property structure, but string priority)
        let flat_node = Node::new(
            "task".to_string(),
            "Flat format task".to_string(),
            json!({
                "status": "done",
                "priority": "low"
            }),
        );

        let task = TaskNode::from_node(flat_node).unwrap();
        assert_eq!(task.content, "Flat format task");
        assert_eq!(task.status.as_str(), "done");
        assert_eq!(task.priority, Some(TaskPriority::Low));
    }

    #[test]
    fn test_task_node_from_node_string_priority() {
        use crate::models::{TaskNode, TaskPriority};

        // Test string priority - now the canonical format
        let string_priority_node = Node::new(
            "task".to_string(),
            "High priority task".to_string(),
            json!({
                "task": {
                    "status": "open",
                    "priority": "high"
                }
            }),
        );

        let task = TaskNode::from_node(string_priority_node).unwrap();
        assert_eq!(task.priority, Some(TaskPriority::High));

        // Test core string priorities
        let low_node = Node::new(
            "task".to_string(),
            "Low".to_string(),
            json!({"priority": "low"}),
        );
        let low_task = TaskNode::from_node(low_node).unwrap();
        assert_eq!(low_task.priority, Some(TaskPriority::Low));

        // Test user-defined priority (schema extensibility)
        let urgent_node = Node::new(
            "task".to_string(),
            "Urgent".to_string(),
            json!({"priority": "urgent"}),
        );
        let urgent_task = TaskNode::from_node(urgent_node).unwrap();
        assert_eq!(
            urgent_task.priority,
            Some(TaskPriority::User("urgent".to_string())),
            "Unknown priorities become User-defined"
        );
    }
}
