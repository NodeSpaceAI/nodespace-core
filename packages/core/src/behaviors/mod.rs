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

use crate::models::{Node, ValidationError as NodeValidationError};
use std::collections::HashMap;
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
    ///     None,
    ///     json!({"status": "pending"}),
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
    /// assert_eq!(defaults["status"], "pending");
    /// ```
    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({})
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
///     None,
///     json!({}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct TextNodeBehavior;

impl NodeBehavior for TextNodeBehavior {
    fn type_name(&self) -> &'static str {
        "text"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Backend validates data integrity: empty nodes are rejected.
        // Frontend manages placeholder UX: empty nodes stay in memory until content added.
        //
        // Architecture:
        // - Frontend: Creates placeholder nodes in _nodes Map when user presses Enter
        // - Frontend: Only sends nodes to backend AFTER user adds content
        // - Backend: Enforces data integrity by rejecting empty content
        //
        // This separation ensures:
        // 1. Good UX: Users can create nodes via Enter key
        // 2. Data integrity: Database only contains meaningful content
        // 3. Clear contract: Frontend handles placeholders, backend validates data
        //
        // Unicode Validation:
        // - Uses is_empty_or_whitespace() to handle Unicode whitespace (U+200B, U+00A0, etc.)
        // - Rust's char.is_whitespace() correctly identifies all Unicode whitespace
        if is_empty_or_whitespace(&node.content) {
            return Err(NodeValidationError::MissingField(
                "Text nodes must have content".to_string(),
            ));
        }
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
///     None,
///     json!({"headerLevel": 2}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct HeaderNodeBehavior;

impl NodeBehavior for HeaderNodeBehavior {
    fn type_name(&self) -> &'static str {
        "header"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Header nodes must have content (same validation as text nodes)
        if is_empty_or_whitespace(&node.content) {
            return Err(NodeValidationError::MissingField(
                "Header nodes must have content".to_string(),
            ));
        }
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
/// # Valid Status Values
///
/// - "pending" - Not started
/// - "in_progress" - Currently being worked on
/// - "completed" - Finished
/// - "cancelled" - No longer needed
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
///     None,
///     json!({"status": "in_progress"}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct TaskNodeBehavior;

impl NodeBehavior for TaskNodeBehavior {
    fn type_name(&self) -> &'static str {
        "task"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        if node.content.trim().is_empty() {
            return Err(NodeValidationError::MissingField(
                "Task nodes must have a description".to_string(),
            ));
        }

        // NOTE: Status validation is now handled by the schema system
        // The schema defines valid enum values dynamically, allowing user customization
        // We only verify it's a string type here
        if let Some(status) = node.properties.get("status") {
            status.as_str().ok_or_else(|| {
                NodeValidationError::InvalidProperties("Status must be a string".to_string())
            })?;
        }

        // Validate priority if present
        if let Some(priority) = node.properties.get("priority") {
            if !priority.is_i64() && !priority.is_null() {
                return Err(NodeValidationError::InvalidProperties(
                    "Priority must be a number".to_string(),
                ));
            }
        }

        Ok(())
    }

    fn can_have_children(&self) -> bool {
        true // Tasks can have subtasks
    }

    fn supports_markdown(&self) -> bool {
        false // Tasks are usually single-line descriptions
    }

    fn default_metadata(&self) -> serde_json::Value {
        serde_json::json!({
            "status": "pending",
            "priority": 2,
            "due_date": null,
            "assignee_id": null
        })
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
///     None,
///     json!({"language": "javascript"}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct CodeBlockNodeBehavior;

impl NodeBehavior for CodeBlockNodeBehavior {
    fn type_name(&self) -> &'static str {
        "code-block"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Code blocks must have content
        // Note: Frontend manages empty placeholders, backend rejects truly empty content
        if is_empty_or_whitespace(&node.content) {
            return Err(NodeValidationError::MissingField(
                "Code block nodes must have content".to_string(),
            ));
        }
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
///     None,
///     json!({}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct QuoteBlockNodeBehavior;

impl NodeBehavior for QuoteBlockNodeBehavior {
    fn type_name(&self) -> &'static str {
        "quote-block"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Quote blocks must have actual content beyond the "> " prefix
        // Strip "> " or ">" from all lines and check if any content remains
        let content_without_prefix: String = node
            .content
            .lines()
            .map(|line| {
                // Remove "> " or ">" from start of each line using strip_prefix
                line.strip_prefix("> ")
                    .or_else(|| line.strip_prefix('>'))
                    .unwrap_or(line)
            })
            .collect::<Vec<&str>>()
            .join("\n");

        // Check if there's any actual content after stripping prefixes
        if is_empty_or_whitespace(&content_without_prefix) {
            return Err(NodeValidationError::MissingField(
                "Quote block nodes must have content beyond the '> ' prefix".to_string(),
            ));
        }
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
///     None,
///     json!({}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct OrderedListNodeBehavior;

impl NodeBehavior for OrderedListNodeBehavior {
    fn type_name(&self) -> &'static str {
        "ordered-list"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // ARCHITECTURAL DECISION: Allow ordered list placeholders ("1. " with no content)
        //
        // Unlike TextNode and HeaderNode which reject empty content after stripping whitespace,
        // OrderedListNode accepts "1. " as valid placeholder content because:
        //
        // 1. The "1. " prefix is STRUCTURAL SYNTAX, not user content
        //    - Similar to how markdown "# " is structural for headers
        //    - The prefix defines the node type and formatting
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
        // This differs from TextNode/HeaderNode where empty content has no semantic meaning.
        // For ordered lists, "1. " represents a valid empty list item in the list structure.
        //
        // Validation: Content must have SOME characters (at minimum the "1. " prefix)
        if node.content.is_empty() {
            return Err(NodeValidationError::MissingField(
                "Ordered list nodes must have content (at least '1. ' prefix)".to_string(),
            ));
        }
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
/// The content field must also match the date ID.
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
///     "2025-01-03".to_string(),
///     None,
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

        // Content should match the date ID
        if node.content != node.id {
            return Err(NodeValidationError::InvalidProperties(
                "Date node content should match the date ID".to_string(),
            ));
        }

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
}

/// Schema node behavior
///
/// Schema nodes store entity type definitions using the Pure JSON schema-as-node pattern.
/// By convention, schema nodes have `id = type_name` and `node_type = "schema"`.
///
/// Schema nodes are validated minimally - they just need non-empty content and valid
/// properties JSON. The SchemaService handles detailed schema validation.
pub struct SchemaNodeBehavior;

impl NodeBehavior for SchemaNodeBehavior {
    fn type_name(&self) -> &'static str {
        "schema"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Basic validation - non-empty content
        if is_empty_or_whitespace(&node.content) {
            return Err(NodeValidationError::MissingField(
                "Schema nodes must have content (schema name)".to_string(),
            ));
        }

        // Properties should be valid JSON object (detailed validation is in SchemaService)
        if !node.properties.is_object() {
            return Err(NodeValidationError::InvalidProperties(
                "Schema properties must be a JSON object".to_string(),
            ));
        }

        Ok(())
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

/// Registry for managing node behaviors
///
/// The registry provides thread-safe storage and retrieval of node behaviors.
/// Built-in behaviors (text, task, date) are registered automatically.
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
///     None,
///     json!({}),
/// );
/// assert!(registry.validate_node(&node).is_ok());
///
/// // Get all registered types
/// let types = registry.get_all_types();
/// assert!(types.contains(&"text".to_string()));
/// assert!(types.contains(&"task".to_string()));
/// assert!(types.contains(&"date".to_string()));
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
    /// Returns `ValidationError::InvalidNodeType` if no behavior is registered
    /// for the node's type, or any validation error from the behavior's `validate()` method.
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
    ///     None,
    ///     json!({}),
    /// );
    /// assert!(registry.validate_node(&valid_node).is_ok());
    ///
    /// let invalid_node = Node::new(
    ///     "unknown_type".to_string(),
    ///     "Content".to_string(),
    ///     None,
    ///     json!({}),
    /// );
    /// assert!(registry.validate_node(&invalid_node).is_err());
    /// ```
    pub fn validate_node(&self, node: &Node) -> Result<(), NodeValidationError> {
        let behavior = self.get(&node.node_type).ok_or_else(|| {
            NodeValidationError::InvalidNodeType(format!(
                "Unknown node type: {}. No behavior registered.",
                node.node_type
            ))
        })?;

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
        let valid_node = Node::new(
            "text".to_string(),
            "Hello world".to_string(),
            None,
            json!({}),
        );
        assert!(behavior.validate(&valid_node).is_ok());

        // Invalid: empty content (backend rejects empty nodes)
        let mut empty_node = valid_node.clone();
        empty_node.content = "".to_string();
        assert!(behavior.validate(&empty_node).is_err());

        // Invalid: whitespace-only content
        let mut whitespace_node = valid_node.clone();
        whitespace_node.content = "   ".to_string();
        assert!(behavior.validate(&whitespace_node).is_err());
    }

    #[test]
    fn test_text_node_unicode_whitespace_validation() {
        let behavior = TextNodeBehavior;
        let base_node = Node::new("text".to_string(), "Valid".to_string(), None, json!({}));

        // Zero-width space (U+200B) - should be rejected
        let mut node = base_node.clone();
        node.content = "\u{200B}".to_string();
        assert!(
            behavior.validate(&node).is_err(),
            "Zero-width space should be rejected"
        );

        // Zero-width non-joiner (U+200C) - should be rejected
        node.content = "\u{200C}".to_string();
        assert!(
            behavior.validate(&node).is_err(),
            "Zero-width non-joiner should be rejected"
        );

        // Zero-width joiner (U+200D) - should be rejected
        node.content = "\u{200D}".to_string();
        assert!(
            behavior.validate(&node).is_err(),
            "Zero-width joiner should be rejected"
        );

        // Non-breaking space (U+00A0) - should be rejected
        node.content = "\u{00A0}".to_string();
        assert!(
            behavior.validate(&node).is_err(),
            "Non-breaking space should be rejected"
        );

        // Line separator (U+2028) - should be rejected
        node.content = "\u{2028}".to_string();
        assert!(
            behavior.validate(&node).is_err(),
            "Line separator should be rejected"
        );

        // Paragraph separator (U+2029) - should be rejected
        node.content = "\u{2029}".to_string();
        assert!(
            behavior.validate(&node).is_err(),
            "Paragraph separator should be rejected"
        );

        // Mixed Unicode whitespace - should be rejected
        node.content = "\u{200B}\u{00A0}\u{2028}".to_string();
        assert!(
            behavior.validate(&node).is_err(),
            "Mixed Unicode whitespace should be rejected"
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
    fn test_task_node_behavior_validation() {
        let behavior = TaskNodeBehavior;

        // Valid task with status
        let valid_node = Node::new(
            "task".to_string(),
            "Implement feature".to_string(),
            None,
            json!({"status": "in_progress"}),
        );
        assert!(behavior.validate(&valid_node).is_ok());

        // Valid task with all fields
        let complete_node = Node::new(
            "task".to_string(),
            "Complete task".to_string(),
            None,
            json!({
                "status": "completed",
                "priority": 3,
                "due_date": "2025-01-10"
            }),
        );
        assert!(behavior.validate(&complete_node).is_ok());

        // Invalid: empty content
        let mut invalid_node = valid_node.clone();
        invalid_node.content = String::new();
        assert!(behavior.validate(&invalid_node).is_err());

        // Invalid: bad status
        let bad_status_node = Node::new(
            "task".to_string(),
            "Task".to_string(),
            None,
            json!({"status": "invalid_status"}),
        );
        assert!(behavior.validate(&bad_status_node).is_err());

        // Invalid: priority not a number
        let bad_priority_node = Node::new(
            "task".to_string(),
            "Task".to_string(),
            None,
            json!({"priority": "high"}),
        );
        assert!(behavior.validate(&bad_priority_node).is_err());
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

        assert_eq!(metadata["status"], "pending");
        assert_eq!(metadata["priority"], 2);
        assert!(metadata["due_date"].is_null());
        assert!(metadata["assignee_id"].is_null());
    }

    #[test]
    fn test_date_node_behavior_validation() {
        let behavior = DateNodeBehavior;

        // Valid date node
        let valid_node = Node::new_with_id(
            "2025-01-03".to_string(),
            "date".to_string(),
            "2025-01-03".to_string(),
            None,
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

        // Invalid: content doesn't match ID
        let mut invalid_content = valid_node.clone();
        invalid_content.content = "2025-01-04".to_string();
        assert!(behavior.validate(&invalid_content).is_err());
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
        let text_node = Node::new("text".to_string(), "Hello".to_string(), None, json!({}));
        assert!(registry.validate_node(&text_node).is_ok());

        // Valid task node
        let task_node = Node::new(
            "task".to_string(),
            "Do something".to_string(),
            None,
            json!({"status": "pending"}),
        );
        assert!(registry.validate_node(&task_node).is_ok());

        // Unknown node type
        let unknown_node = Node::new(
            "unknown".to_string(),
            "Content".to_string(),
            None,
            json!({}),
        );
        let result = registry.validate_node(&unknown_node);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(NodeValidationError::InvalidNodeType(_))
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

                let node = Node::new(
                    "text".to_string(),
                    "Thread test".to_string(),
                    None,
                    json!({}),
                );
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

        let node = Node::new("text".to_string(), "Test".to_string(), None, json!({}));
        assert!(behavior.validate(&node).is_ok());
    }
}
