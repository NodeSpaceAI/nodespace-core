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
use crate::models::CoreNodeType;
use crate::models::{
    Node, PlayFields, QueryFields, SchemaNode, SkillFields, ValidationError as NodeValidationError,
};
use crate::services::NodeAccessor;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use thiserror::Error;

/// Get a property value, checking namespaced location first, then flat.
///
/// Namespaced format (`properties.{namespace}.key`) takes precedence so that
/// MCP updates — which always normalize to namespaced format — override stale
/// flat values left from pre-normalization databases.
fn get_namespaced_prop<'a>(properties: &'a Value, namespace: &str, key: &str) -> Option<&'a Value> {
    properties
        .get(namespace)
        .and_then(|ns| ns.get(key))
        .or_else(|| properties.get(key))
}

/// Get a string property, checking namespaced location first, then flat.
fn get_namespaced_prop_str<'a>(
    properties: &'a Value,
    namespace: &str,
    key: &str,
) -> Option<&'a str> {
    get_namespaced_prop(properties, namespace, key).and_then(|v| v.as_str())
}

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

    /// Phase 2: Optional async — aggregated content from related nodes
    ///
    /// Called by the embedding service after `get_embeddable_content()` returns `Some`.
    /// Enables tree-walking types (text, header) to fetch children via `NodeAccessor`
    /// and concatenate their contributions into the parent's embedding.
    ///
    /// Default returns `None` — most types don't need child aggregation.
    ///
    /// # Arguments
    ///
    /// * `node` - The root node being embedded
    /// * `accessor` - Read-only node accessor for fetching related nodes
    fn get_aggregated_content<'a>(
        &'a self,
        _node: &'a Node,
        _accessor: &'a dyn NodeAccessor,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async { None })
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

const MAX_AGGREGATION_DEPTH: usize = 20;

/// Recursively collect content from a node's children for embedding aggregation.
///
/// Performs a pre-order (document-order) depth-first traversal via
/// `NodeAccessor::get_children()`, collecting each child's
/// `get_parent_contribution()` output in the order a human reads the
/// document top to bottom: a child's own contribution comes first, followed
/// by the full contents of its subtree, before its next sibling is visited.
/// Limits depth to prevent runaway traversal on deeply nested trees.
///
/// Never spans an access boundary (ADR-059 §7). A non-person descendant filed
/// into a collection of its own (holding a `member_of` edge) breaks ADR-059
/// §2 and is a defect: it is logged, and it and its subtree are left out. It is
/// embedded as its own root instead. If the boundaries cannot be read, nothing
/// is aggregated rather than risking a leak.
///
/// An archived descendant is left out, with its subtree: archiving takes a
/// node out of the vector index (ADR-087 §2), and for a child that index is
/// its root's vector.
///
/// Used by text and header behaviors for `get_aggregated_content()`.
async fn aggregate_children_content(
    node: &Node,
    accessor: &dyn NodeAccessor,
    registry: &NodeBehaviorRegistry,
) -> Option<String> {
    let boundaries = match accessor.access_boundaries_under(&node.id).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(
                root_id = %node.id,
                error = %e,
                "failed to read access boundaries; aggregating no descendants"
            );
            return None;
        }
    };
    let mut parts = Vec::new();

    // Stack of (node, depth) entries not yet visited. Seeded with `node`'s
    // direct children pushed in REVERSE sibling order, so popping (LIFO)
    // visits them first-child-first. Each pop immediately records that
    // node's contribution, then pushes ITS OWN children (again reversed)
    // before any sibling further down the stack is reached — draining a
    // child's whole subtree before moving to its next sibling, i.e.
    // pre-order / document order, not level-by-level.
    let root_children = match accessor.get_children(&node.id).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to get children for {}: {}", node.id, e);
            Vec::new()
        }
    };
    let mut stack: Vec<(Node, usize)> = root_children.into_iter().rev().map(|c| (c, 1)).collect();

    while let Some((child, depth)) = stack.pop() {
        if boundaries.contains(&child.id) {
            tracing::error!(
                root_id = %node.id,
                descendant_id = %child.id,
                "ADR-059 §7 defect: descendant is filed into a collection of its own; \
                 excluded from the root's embedding and embedded as its own root"
            );
            continue;
        }
        if !crate::governance::participates(&child) {
            continue;
        }
        // Use behavior to get the contribution this child makes to its parent's
        // embedding, resolved through the child's `extends` chain so a subtype
        // contributes as the type it extends does.
        let chain = match accessor.type_chain(&child.node_type).await {
            Ok(chain) => chain,
            Err(e) => {
                tracing::warn!(
                    "Failed to resolve the type chain of {}: {}",
                    child.node_type,
                    e
                );
                vec![child.node_type.clone()]
            }
        };
        let behavior = registry.resolve(&chain);
        if let Some(contribution) = behavior.get_parent_contribution(&child) {
            parts.push(contribution);
        }
        // Recurse into this child's own children (depth-first) before its
        // siblings, unless there is nothing below it to aggregate: its type
        // takes no children (ADR-089), or its subtree is not embedded and its
        // children are embedding roots of their own.
        let ends_here = crate::models::CoreNodeType::nearest(&chain).is_some_and(|core| {
            core.structure().children == crate::models::ChildrenRule::None || !core.embeds_subtree()
        });
        if depth >= MAX_AGGREGATION_DEPTH || ends_here {
            continue;
        }
        let grandchildren = match accessor.get_children(&child.id).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Failed to get children for {}: {}", child.id, e);
                continue;
            }
        };
        stack.extend(grandchildren.into_iter().rev().map(|gc| (gc, depth + 1)));
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
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
        // Phase 1: Allow blank text nodes
        // Changed behavior: Backend now accepts blank text nodes
        //
        // Architecture Change:
        // - Frontend: Persists blank nodes immediately (with 500ms debounce)
        // - Backend: Accepts blank text nodes (user responsible for managing them)
        // - User Experience: Users can create blank nodes via Enter key, burden is on user to maintain or delete
        //
        // This change:
        // 1. Eliminates ephemeral-during-editing behavior
        // 2. Prevents UNIQUE constraint violations when indenting blank nodes
        // 3. Simplifies frontend persistence logic (Phase 2 will remove deferred update queue)
        //
        // Previous behavior (before blank content was allowed):
        // - Backend rejected blank nodes with validation error
        // - Frontend had to manage ephemeral nodes until content was added
        // - Caused database constraint issues during indent operations

        // Blank text nodes are now allowed - no validation required
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        true
    }

    /// Text nodes aggregate children's content for embedding
    fn get_aggregated_content<'a>(
        &'a self,
        node: &'a Node,
        accessor: &'a dyn NodeAccessor,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async move {
            let registry = NodeBehaviorRegistry::new();
            aggregate_children_content(node, accessor, &registry).await
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
        // Allow blank header nodes (e.g., "##" with no content)
        // Similar to text nodes, headers can be created blank and filled in later
        // Frontend manages the UX of blank headers (e.g., showing placeholder text)
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        true
    }

    /// Header nodes aggregate children's content for embedding
    fn get_aggregated_content<'a>(
        &'a self,
        node: &'a Node,
        accessor: &'a dyn NodeAccessor,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async move {
            let registry = NodeBehaviorRegistry::new();
            aggregate_children_content(node, accessor, &registry).await
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
/// All values use lowercase format for consistency across layers.
///
/// Core values (protected, cannot be removed):
/// - "open" - Not started (default)
/// - "in_progress" - Currently being worked on
/// - "done" - Finished
/// - "cancelled" - Cancelled/abandoned
///
/// User-extensible values can be added via schema.
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

impl NodeBehavior for TaskNodeBehavior {
    fn type_name(&self) -> &'static str {
        "task"
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        // A task's field vocabulary is the schema's to check, and empty
        // content is allowed. The behaviour also runs for every type that
        // extends `task`, so it never reads the node's own type.
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false // Tasks are usually single-line descriptions
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

/// True if `s` is an ISO calendar date `YYYY-MM-DD` (zero-padded), so plain
/// string comparison orders two such dates correctly. Anything else returns
/// false, in which case the project date-range check is skipped (per spec:
/// enforce only when both dates parse as ISO).
fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, &c)| {
            if i == 4 || i == 7 {
                c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

/// Built-in behavior for project nodes
///
/// A ProjectNode (`node_type = "project"`) is a container for tasks, milestones,
/// and related work. Its name is the node `content` (like Collection and Task);
/// typed data lives under the `properties.project.*` namespace. Children are real
/// graph nodes attached via `has_child` edges, and ownership/membership are graph
/// edges — never property blobs (Universal Graph model).
///
/// Validation split: this behavior does TYPE checks plus the cross-field
/// `start_date <= end_date` rule; the schema system validates enum membership
/// and values, so enum-value checks are not duplicated here.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, ProjectNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = ProjectNodeBehavior;
/// let node = Node::new(
///     "project".to_string(),
///     "Launch v1".to_string(),
///     json!({ "project": { "status": "active" } }),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct ProjectNodeBehavior;

impl NodeBehavior for ProjectNodeBehavior {
    fn type_name(&self) -> &'static str {
        "project"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Project name is the node content — required.
        if node.content.trim().is_empty() {
            return Err(NodeValidationError::MissingField(
                "content (project name)".to_string(),
            ));
        }

        // Typed fields live under `properties.project.*` — `project` is a
        // schema-typed node type, so NodeService hoists its schema-defined
        // fields there on write, the same as `task` under `properties.task.*`.
        // Falling back to the flat
        // top level keeps this correct for a write that does not normalize
        // flat properties (`bulk_create`). TYPE checks only — the schema system validates enum
        // membership and allowed values.
        let project_props = node
            .properties
            .get("project")
            .filter(|v| v.is_object())
            .or(Some(&node.properties));
        if let Some(props) = project_props {
            // status / priority must be strings if present.
            for field in ["status", "priority"] {
                if let Some(v) = props.get(field) {
                    if !v.is_string() && !v.is_null() {
                        return Err(NodeValidationError::InvalidProperties(format!(
                            "{} must be a string",
                            field
                        )));
                    }
                }
            }

            // start_date / end_date must be strings if present; when both are
            // present and both parse as ISO YYYY-MM-DD, enforce start <= end
            // (string comparison is valid for zero-padded ISO dates).
            let read_date = |name: &str| -> Result<Option<String>, NodeValidationError> {
                match props.get(name) {
                    None | Some(serde_json::Value::Null) => Ok(None),
                    Some(v) => match v.as_str() {
                        Some(s) => Ok(Some(s.to_string())),
                        None => Err(NodeValidationError::InvalidProperties(format!(
                            "{} must be a string",
                            name
                        ))),
                    },
                }
            };
            let start = read_date("start_date")?;
            let end = read_date("end_date")?;
            if let (Some(s), Some(e)) = (&start, &end) {
                if is_iso_date(s) && is_iso_date(e) && s > e {
                    return Err(NodeValidationError::InvalidProperties(
                        "start_date must be on or before end_date".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        true // Projects carry rich descriptions
    }

    /// Projects are not embedded, like tasks.
    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    /// Projects don't contribute to a parent's embedding either.
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
        // Allow blank code blocks (e.g., "```language" with no code)
        // Users can create blank code blocks and fill in code later
        // Frontend manages the UX of blank code blocks (e.g., showing placeholder text)
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false // Code blocks display raw text, no markdown formatting
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
        // Allow blank quote blocks (e.g., ">" with no content)
        // Users can create blank quote blocks and fill in quoted text later
        // Frontend manages the UX of blank quote blocks (e.g., showing placeholder text)
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        true // Quote blocks support inline markdown formatting
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
        // Allow blank ordered list nodes (consistent with headers, quotes, etc.)
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
        // 4. Consistency with other node types
        //    - Headers allow blank content after "##"
        //    - Quote blocks allow blank content after ">"
        //    - Code blocks allow blank content after "```"
        //    - Ordered lists should allow blank content after "1. "
        //
        // Frontend manages the UX of blank ordered list nodes (e.g., showing placeholder text)
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        true // Ordered lists support inline markdown formatting
    }
}

/// Built-in behavior for horizontal line (thematic break) nodes
///
/// Horizontal lines are decorative elements that don't carry semantic content.
/// They cannot have children and don't contribute to embeddings.
pub struct HorizontalLineNodeBehavior;

impl NodeBehavior for HorizontalLineNodeBehavior {
    fn type_name(&self) -> &'static str {
        "horizontal-line"
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false
    }

    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None // Decorative element, no semantic content
    }

    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Built-in behavior for table nodes
///
/// Tables store GFM markdown table content. They cannot have children
/// but do contribute to embeddings (searchable text content).
pub struct TableNodeBehavior;

impl NodeBehavior for TableNodeBehavior {
    fn type_name(&self) -> &'static str {
        "table"
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false
    }

    fn get_embeddable_content(&self, node: &Node) -> Option<String> {
        if node.content.trim().is_empty() {
            None
        } else {
            Some(node.content.clone())
        }
    }

    fn get_parent_contribution(&self, node: &Node) -> Option<String> {
        if node.content.trim().is_empty() {
            None
        } else {
            Some(node.content.clone())
        }
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
/// The content field can be any custom content (no longer
/// required to match the date ID).
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

        // NOTE: Date nodes can have custom content (not required to match ID).
        // The ID is always in YYYY-MM-DD format, but content can be anything (e.g., "Custom Date Content").

        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false // Dates are simple identifiers
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
/// - Field names must be unique (alphanumeric and underscores, with an optional
///   `<namespace>:` prefix — see [`validate_schema_field_name`])
/// - Enum fields must have at least one value defined (in core_values or user_values)
///
/// # Strongly-Typed Validation
///
/// This behavior supports both generic Node validation (via `validate()`) and
/// strongly-typed SchemaNode validation (via `validate_schema_node()`). The generic
/// validation internally converts to SchemaNode for type-safe validation.
pub struct SchemaNodeBehavior;

/// Validate a schema field name.
///
/// A field name is either a bare name (`capacity`) or a namespaced name carrying
/// exactly one `<namespace>:` prefix (`custom:capacity`). Both the namespace and
/// the bare name must be non-empty and contain only alphanumerics and underscores.
///
/// Both forms are well-formed, and which one is *required* depends on the type
/// being described rather than on the name alone: fields of a user-defined type
/// are stored bare, while extending a core type requires a prefix so a future
/// core property cannot collide with a user's field. Neither `create_schema`
/// route adds a prefix — both store the name they are given, so the two routes
/// produce identical stored keys.
///
/// This function therefore decides only whether a name is *well-formed*. The
/// core-type prefix requirement is enforced where the target schema is known
/// (`update_schema`'s `add_fields` path); it cannot be enforced here, which sees
/// a name with no indication of the type it belongs to. Rejecting either form
/// here would reject names the rest of the system handles correctly: CEL strips
/// the prefix, the graph resolver matches with and without it, and identifier
/// validation in the query path permits it.
///
/// A leading `_` is the one exception, and it is rejected rather than warned
/// about. `_` marks internal bookkeeping on both sides of the property
/// round-trip: the write path leaves such keys outside the type's namespace,
/// and `flatten_namespaced_properties` drops them from every read surface
/// unconditionally. A field declared with that prefix could therefore be
/// written but never read back — accepting it would store data that is
/// unreachable by construction, so this is a hard error rather than a warning
/// like the reserved-core-property collision (which stores a field that does
/// work, and only *may* be shadowed later).
///
/// The check is on the stored key, which is the full name verbatim, so only a
/// leading `_` on the whole name is fatal: `custom:_internal` stores under a
/// key beginning `c`, survives the flattener, and stays legal. The rejection
/// names that alternative, so it redirects rather than merely refuses — a
/// caller reaching for `_internal` wants a private-looking field, and the
/// namespaced form gives them one that actually round-trips.
pub(crate) fn validate_schema_field_name(name: &str) -> Result<(), NodeValidationError> {
    let invalid = |reason: &str| {
        Err(NodeValidationError::InvalidProperties(format!(
            "Invalid field name '{}': {}",
            name, reason
        )))
    };

    if name.starts_with('_') {
        // Name the legal alternative rather than only the rule: a caller
        // reaching for `_internal` wants a private-looking field, and a
        // namespaced name is exactly that and does round-trip. Built from the
        // caller's own name so the suggestion is theirs to paste, not a
        // generic example they have to translate — except for an all-underscore
        // name, which leaves nothing to suggest and falls back to the rule
        // alone rather than proposing the equally invalid 'custom:'.
        let suggestion = name.trim_start_matches('_');
        let remedy = if suggestion.is_empty() {
            "Use a '<namespace>:' prefix instead, e.g. 'custom:name'.".to_string()
        } else {
            format!(
                "Use a '<namespace>:' prefix instead, e.g. 'custom:{}', which round-trips normally.",
                suggestion
            )
        };
        return invalid(&format!(
            "a leading '_' is reserved for internal bookkeeping — such a field is \
             dropped from every read path, so it could be written but never read \
             back. {remedy}"
        ));
    }

    let is_valid_segment =
        |s: &str| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_');

    let mut parts = name.split(':');
    // `split` always yields at least one element, so this cannot panic.
    let bare_name = match (parts.next(), parts.next(), parts.next()) {
        // Bare name: `capacity`
        (Some(bare), None, _) => bare,
        // Namespaced name: `custom:capacity`
        (Some(namespace), Some(bare), None) => {
            if !is_valid_segment(namespace) {
                return invalid(
                    "namespace prefix must contain only alphanumeric characters and underscores",
                );
            }
            bare
        }
        // More than one ':' — e.g. `custom:a:b`
        _ => return invalid("must contain at most one ':' namespace prefix"),
    };

    if !is_valid_segment(bare_name) {
        return invalid(
            "must contain only alphanumeric characters and underscores, \
             optionally preceded by a '<namespace>:' prefix",
        );
    }

    Ok(())
}

/// Validate a single schema field (standalone function for recursive validation)
fn validate_schema_field(field: &SchemaField) -> Result<(), NodeValidationError> {
    validate_schema_field_name(&field.name)?;

    // Enum fields must have at least one value defined
    if field.field_type == crate::models::SchemaFieldType::Enum {
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

/// Validate `{token}` syntax in a template string against a set of defined field names.
///
/// Every `{field_name}` token must:
/// - Have a matching closing `}`
/// - Not be empty (`{}` is invalid)
/// - Reference a field that exists in `defined_fields`
fn validate_template_tokens(
    template: &str,
    defined_fields: &HashSet<&str>,
    template_field: &str,
) -> Result<(), NodeValidationError> {
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            match bytes[i + 1..].iter().position(|&c| c == b'}') {
                None => {
                    return Err(NodeValidationError::InvalidProperties(format!(
                        "{} contains an unclosed '{{' placeholder",
                        template_field
                    )));
                }
                Some(end) => {
                    let field_name = &template[i + 1..i + 1 + end];
                    if field_name.is_empty() {
                        return Err(NodeValidationError::InvalidProperties(format!(
                            "{} contains an empty '{{}}' placeholder",
                            template_field
                        )));
                    }
                    if !defined_fields.contains(field_name) {
                        return Err(NodeValidationError::InvalidProperties(format!(
                            "{} references undefined field '{}' — add it to schema.fields",
                            template_field, field_name
                        )));
                    }
                    i += 1 + end + 1; // skip past '}'
                    continue;
                }
            }
        }
        i += 1;
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
        if is_empty_or_whitespace(&schema.envelope.content) {
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

        let defined_fields: HashSet<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();

        // Validate title_template syntax: every '{' must have a matching '}' and a non-empty field name
        if let Some(template) = &schema.title_template {
            validate_template_tokens(template, &defined_fields, "title_template")?;
        }

        // Validate properties_header_summary_template with the same rules
        if let Some(template) = &schema.properties_header_summary_template {
            validate_template_tokens(
                template,
                &defined_fields,
                "properties_header_summary_template",
            )?;
        }

        Ok(())
    }
}

impl SchemaNodeBehavior {
    /// A schema's `children` or `parent` property, when present, must be a
    /// rule of the declared shape, and a rule that takes a list must name at
    /// least one type.
    ///
    /// This checks shape only, on every path that writes a schema node.
    /// Whether the named types exist and whether the rule only tightens its
    /// base are `create_schema`'s and `update_schema`'s checks: a rule that
    /// names a missing type matches nothing, and enforcement adds a
    /// subtype's rule to its base's, so neither can loosen what is enforced.
    fn validate_structural_rule<R: serde::de::DeserializeOwned>(
        node: &Node,
        key: &str,
    ) -> Result<(), NodeValidationError> {
        let Some(value) = node.properties.get(key) else {
            return Ok(());
        };
        serde_json::from_value::<R>(value.clone()).map_err(|e| {
            NodeValidationError::InvalidProperties(format!(
                "Schema '{}' declares a \"{key}\" rule that is not a structural rule: {e}",
                node.id
            ))
        })?;
        if value
            .get("types")
            .and_then(|types| types.as_array())
            .is_some_and(|types| types.is_empty() || types.iter().any(|t| !t.is_string()))
        {
            return Err(NodeValidationError::InvalidProperties(format!(
                "The \"{key}\" rule of schema '{}' must name at least one type",
                node.id
            )));
        }
        Ok(())
    }
}

impl NodeBehavior for SchemaNodeBehavior {
    fn type_name(&self) -> &'static str {
        "schema"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // A structural rule (ADR-089) the database cannot read is refused
        // rather than read as `any`: the conversion below is lenient, and the
        // rule triggers copy only the shapes they know.
        Self::validate_structural_rule::<crate::models::SchemaChildrenRule>(node, "children")?;
        Self::validate_structural_rule::<crate::models::SchemaParentRule>(node, "parent")?;

        // Validate the row as a typed schema. Only the row is judged here,
        // so it is read with no declarations and goes no further than this
        // check: a schema's relationships and parent are declaration edges,
        // validated where they are written.
        match crate::models::schema_node::from_storage(node.clone(), Vec::new()) {
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

    fn supports_markdown(&self) -> bool {
        false // Schemas are structured data
    }

    /// Schema nodes aggregate their description child subtree for embedding.
    ///
    /// The description is stored as a markdown node subtree under the schema node.
    /// Aggregating it gives semantic search the full descriptive text, enabling
    /// synonym discovery (e.g. "billing" → existing "Invoice" schema).
    fn get_aggregated_content<'a>(
        &'a self,
        node: &'a Node,
        accessor: &'a dyn NodeAccessor,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async move {
            let registry = NodeBehaviorRegistry::new();
            aggregate_children_content(node, accessor, &registry).await
        })
    }
}

/// Built-in behavior for query nodes
///
/// Query nodes store structured query definitions for filtering/searching nodes.
/// Primary use case is AI chat creating queries as child nodes (not manual search UI).
///
/// # Storage Architecture
///
/// Query nodes store all data in the unified `node` table (Universal Graph Architecture):
/// - **Content (`node.content`)**: Plain text description (e.g., "All open high-priority tasks")
/// - **Properties (`node.properties`)**: The query schema's snake_case fields (target_type, filters,
///   sorting, limit, view_config, …), read only through [`QueryFields`]
///
/// # Characteristics
///
/// - Supports markdown: false
/// - Content: Plain text description of the query
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, QueryNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = QueryNodeBehavior;
/// let node = Node::new(
///     "query".to_string(),
///     "All open tasks with high priority".to_string(),
///     json!({}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct QueryNodeBehavior;

impl NodeBehavior for QueryNodeBehavior {
    fn type_name(&self) -> &'static str {
        "query"
    }

    /// Every field must decode through [`QueryFields`], the one reader of a
    /// stored query, so a filter the query service cannot execute or a
    /// non-object `view_config` is rejected on write rather than discovered
    /// when the view is opened.
    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Decoded from the properties rather than the node, so a type
        // extending `query` is held to the same field shapes.
        QueryFields::from_properties(&node.properties)?;
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false
    }

    /// Queries are not embedded as standalone roots
    ///
    /// Query definitions are not semantic content worth embedding.
    /// They're operational/structural nodes for filtering data.
    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    /// Queries don't contribute to parent embeddings
    ///
    /// Query nodes are typically children of chat nodes but shouldn't
    /// pollute the chat's semantic embedding with query syntax.
    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Built-in behavior for collection nodes
///
/// Collection nodes provide hierarchical labels for organizing other nodes.
/// They form a DAG (Directed Acyclic Graph) structure where:
/// - Collections can have multiple parent collections (multi-parent)
/// - Nodes can belong to multiple collections (multi-membership)
/// - Collection names are best-effort unique among ACTIVE collections
///   (case-insensitive lookup, `lifecycle_status = 'active'` only — an
///   archived collection's name is free to reuse): a name collision is never
///   hard-rejected, whether at create time or on a rename (NodeSpace is
///   local-first — two offline devices can each validly create or rename a
///   collection onto the same name), and is instead surfaced as a
///   non-blocking `CollectionNameCollision` conflict-journal record naming
///   both nodes once they land in the same database (ADR-068;
///   `SqliteStore::create_node`, `SqliteStore::update_node`,
///   `SqliteStore::update_node_with_version_check`), mirroring the
///   suggest-don't-block posture ADR-065 established for the schema-declared
///   `unique` rule.
///
/// Collections use the `content` field for the collection name.
///
/// # Path Syntax
///
/// Collections use `:` delimiter for hierarchical paths:
/// - `hr:policy:vacation:Berlin` - Nested path
/// - `engineering:docs` - Simple hierarchy
/// - `Berlin` - Single collection
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, CollectionNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = CollectionNodeBehavior;
/// let node = Node::new(
///     "collection".to_string(),
///     "Engineering".to_string(),
///     json!({}),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct CollectionNodeBehavior;

impl NodeBehavior for CollectionNodeBehavior {
    fn type_name(&self) -> &'static str {
        "collection"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Collection names must be non-empty
        // The content field stores the collection name
        if node.content.trim().is_empty() {
            return Err(NodeValidationError::MissingField(
                "Collection name (content) cannot be empty".to_string(),
            ));
        }

        // Collection names cannot contain the path delimiter ':'
        // This ensures clean path parsing
        if node.content.contains(':') {
            return Err(NodeValidationError::InvalidProperties(
                "Collection name cannot contain ':' (path delimiter)".to_string(),
            ));
        }

        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false // Collection names are plain text
    }

    /// Collections are not embedded (they're organizational containers)
    ///
    /// Collections are organizational labels, not semantic content.
    /// The nodes that are members of collections carry the actual embeddable content.
    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    /// Collections don't contribute to parent embeddings
    ///
    /// Collections are structural/organizational and don't have semantic content
    /// that should be embedded.
    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Behavior for the abstract `ai-chat` base (ADR-088 §1).
///
/// No node has `ai-chat` as its type, but every chat is validated by this
/// behaviour first: a subtype's behaviour adds to it and never replaces it.
///
/// - **Content** is the chat's title, and must be supplied. `"Untitled"`
///   requests automatic titling.
/// - **Not embedded**, and it contributes nothing to a parent's embedding:
///   conversations are not general knowledge and must not surface in semantic
///   search (ADR-061 §4).
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::{NodeBehavior, AiChatNodeBehavior};
/// use nodespace_core::models::Node;
/// use serde_json::json;
///
/// let behavior = AiChatNodeBehavior;
/// let node = Node::new(
///     "ai-chat-native".to_string(),
///     "Implement webhook handler".to_string(),
///     json!({ "agent": "nodespace", "model": "gemma-4-e4b-q4km" }),
/// );
/// assert!(behavior.validate(&node).is_ok());
/// ```
pub struct AiChatNodeBehavior;

impl NodeBehavior for AiChatNodeBehavior {
    fn type_name(&self) -> &'static str {
        "ai-chat"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // A chat's title is its content, and every client must supply one.
        //
        // Omitting a title is an error rather than a request for automatic
        // titling: the background titler claims a chat only when its content
        // is the literal `"Untitled"` sentinel, so opting in is an explicit
        // act a client performs by writing that value. Were empty content
        // accepted here, any client that created a chat without a title —
        // over the generic `create_node` RPC, say — would be silently opted
        // into titling behaviour scoped to the desktop UI.
        //
        // Uses `is_empty_or_whitespace` rather than `trim()`: a title made of
        // zero-width characters is not a title, and would otherwise pass here
        // and then fail to match the sentinel, leaving a chat that looks
        // blank and can never be auto-titled.
        if is_empty_or_whitespace(&node.content) {
            return Err(NodeValidationError::MissingField(
                "content (chat title; use \"Untitled\" to request automatic titling)".to_string(),
            ));
        }

        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false // Chat content is rendered by the chat viewer, not the markdown pipeline
    }

    /// Chats are intentionally NOT embedded, whatever they hold.
    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    /// A chat doesn't contribute to a parent's embedding either: it shouldn't
    /// pollute the embedding of the page it sits under.
    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// The rules a chat subtype's behaviour takes from the abstract base.
///
/// [`NodeBehaviorRegistry::resolve`] answers embedding, markdown and children
/// questions from the nearest behaviour in a node's chain, which for a chat is
/// its subtype's. Each subtype behaviour hands these back to
/// [`AiChatNodeBehavior`] through this macro, so a chat subtype cannot come to
/// be embedded by leaving a method out.
macro_rules! inherit_ai_chat_rules {
    () => {
        fn supports_markdown(&self) -> bool {
            AiChatNodeBehavior.supports_markdown()
        }

        fn get_embeddable_content(&self, node: &Node) -> Option<String> {
            AiChatNodeBehavior.get_embeddable_content(node)
        }

        fn get_parent_contribution(&self, node: &Node) -> Option<String> {
            AiChatNodeBehavior.get_parent_contribution(node)
        }
    };
}

/// Behavior for `ai-chat-native` nodes: a conversation run by NodeSpace's
/// agent loop. Adds nothing to [`AiChatNodeBehavior`]: its fields are scalars
/// the schema validates, and its messages are its `ai-chat-message` children.
pub struct AiChatNativeNodeBehavior;

impl NodeBehavior for AiChatNativeNodeBehavior {
    fn type_name(&self) -> &'static str {
        "ai-chat-native"
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        Ok(())
    }

    inherit_ai_chat_rules!();
}

/// Behavior for `ai-chat-pty` nodes: an external coding agent in a terminal.
/// Adds nothing to [`AiChatNodeBehavior`]: its fields are scalars the schema
/// validates.
pub struct AiChatPtyNodeBehavior;

impl NodeBehavior for AiChatPtyNodeBehavior {
    fn type_name(&self) -> &'static str {
        "ai-chat-pty"
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        Ok(())
    }

    inherit_ai_chat_rules!();
}

/// Behavior for `ai-chat-message` nodes: one message of a native chat
/// (ADR-088 §3).
///
/// The closed `role` and `outcome` vocabularies are the schema's to enforce.
/// This checks the one rule that spans two fields: only an assistant message
/// records how its turn ended.
///
/// A message is not embedded and contributes nothing to its chat's embedding:
/// conversation fragments are not general knowledge (ADR-061 §4).
pub struct AiChatMessageNodeBehavior;

impl NodeBehavior for AiChatMessageNodeBehavior {
    fn type_name(&self) -> &'static str {
        "ai-chat-message"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        let field = |name: &str| {
            get_namespaced_prop(&node.properties, self.type_name(), name).filter(|v| !v.is_null())
        };
        let assistant = serde_json::json!(crate::models::AiChatMessageRole::Assistant);
        if field("outcome").is_some() && field("role") != Some(&assistant) {
            return Err(NodeValidationError::InvalidProperties(
                "outcome is recorded on an assistant message only".to_string(),
            ));
        }
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false // Rendered by the chat viewer, not the markdown pipeline
    }

    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Behavior for agent-guidance nodes (unconditional base system-prompt sections)
///
/// Agent-guidance root nodes store only a short label in `content`; body
/// content lives in child nodes. `PromptAssembler` fetches children and
/// concatenates their content for assembly, on every turn — unlike `skill`
/// nodes, which are discovered on demand via `search_skills`. All guidance is
/// rendered through Minijinja.
///
/// # Embedding
///
/// Agent-guidance nodes are NOT semantically indexed — they are internal
/// agent infrastructure, not user knowledge content.
pub struct AgentGuidanceNodeBehavior;

impl NodeBehavior for AgentGuidanceNodeBehavior {
    fn type_name(&self) -> &'static str {
        "agent-guidance"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Agent-guidance title (content) should not be empty
        if node.content.trim().is_empty() {
            return Err(NodeValidationError::InvalidProperties(
                "Agent-guidance title (content) cannot be empty".to_string(),
            ));
        }
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        true // Child nodes will be markdown
    }

    /// Agent-guidance nodes are not semantically indexed
    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    /// Agent-guidance nodes don't contribute to parent embeddings
    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Built-in behavior for play nodes (workflow definitions).
///
/// A play's rules are validated by the play engine's own pipeline on every
/// write (`NodeService::validate_play_rules`), and its fields by the schema,
/// so there is nothing left for the behaviour to check. A play is automation,
/// not knowledge: it is not embedded.
pub struct PlayNodeBehavior;

impl NodeBehavior for PlayNodeBehavior {
    fn type_name(&self) -> &'static str {
        CoreNodeType::Play.as_str()
    }

    /// Every field must decode through [`PlayFields`], the one reader of a
    /// stored play, so a rule with an unknown trigger or action, a missing
    /// param, or a param its action does not take is rejected on write
    /// rather than saved and never run.
    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Decoded from the properties rather than the node, so a type
        // extending `play` is held to the same field shapes.
        PlayFields::from_properties(&node.properties)?;
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false
    }

    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Built-in behavior for checkbox nodes.
///
/// A checkbox is a primitive: its text and its checked state are both in
/// `content` (`- [ ] ` / `- [x] `). It is searchable text like any other
/// markup body, so it takes the default embedding rules.
pub struct CheckboxNodeBehavior;

impl NodeBehavior for CheckboxNodeBehavior {
    fn type_name(&self) -> &'static str {
        CoreNodeType::Checkbox.as_str()
    }

    fn validate(&self, _node: &Node) -> Result<(), NodeValidationError> {
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        true
    }
}

/// Built-in behavior for skill nodes (ADR-030 Phase 3)
///
/// Skills define what the agent can do and how. They contain:
/// - A description (drives semantic search for skill discovery)
/// - A tool whitelist (which tools this skill can use)
/// - Max iterations for the ReAct loop
/// - Child prompt nodes containing guidance and examples
///
/// Content holds the skill name (e.g., "Research & Search").
/// The `description` property drives embedding for discovery.
pub struct SkillNodeBehavior;

impl NodeBehavior for SkillNodeBehavior {
    fn type_name(&self) -> &'static str {
        "skill"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        // Content (skill name) should not be empty
        if node.content.trim().is_empty() {
            return Err(NodeValidationError::MissingField(
                "Skill name (content) cannot be empty".to_string(),
            ));
        }

        // Field types (description, exclusion, tool_whitelist,
        // max_iterations) are the model's to check. Decoded from
        // the properties rather than the node, so a type extending `skill`
        // is held to the same field shapes.
        SkillFields::from_properties(&node.properties)?;

        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false
    }

    /// The description property drives embedding for skill discovery
    fn get_embeddable_content(&self, node: &Node) -> Option<String> {
        // A skill that fails to decode was rejected by `validate` on write,
        // so only an in-memory node can reach here malformed; embed its name.
        let description = SkillFields::from_properties(&node.properties)
            .map(|skill| skill.description)
            .unwrap_or_default();
        let desc = description.as_str();
        let name = &node.content;

        if desc.is_empty() && name.trim().is_empty() {
            None
        } else if desc.is_empty() {
            Some(name.clone())
        } else {
            Some(format!("{}\n\n{}", name, desc))
        }
    }

    /// Skills don't contribute to parent embeddings
    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Behavior for the abstract `tool` base (ADR-086 §12).
///
/// No node has `tool` as its type, but every tool is validated by this
/// behaviour first: a subtype's behaviour adds to it and never replaces it.
/// A tool node is a registry entry, metadata only; where it comes from, and
/// what runs when it is called, is its subtype's.
///
/// - **Content** is the tool's name as the model sees it, and must be
///   supplied.
/// - `parameter_schema` is a JSON Schema bounded in depth, with no open
///   `additionalProperties`, whichever subtype stores it.
/// - **Embedded** as its name and `description`, so a tool is found by
///   intent.
pub struct ToolNodeBehavior;

impl NodeBehavior for ToolNodeBehavior {
    fn type_name(&self) -> &'static str {
        CoreNodeType::Tool.as_str()
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        if node.content.trim().is_empty() {
            return Err(NodeValidationError::MissingField(
                "Tool display name (content) cannot be empty".to_string(),
            ));
        }

        if let Some(schema) =
            get_namespaced_prop(&node.properties, self.type_name(), "parameter_schema")
        {
            if !schema.is_object() && !schema.is_null() {
                return Err(NodeValidationError::InvalidProperties(
                    "parameter_schema must be a JSON object".to_string(),
                ));
            }
            // Trust-boundary guard: bound nesting depth and reject unbounded
            // `additionalProperties`. One rule for every subtype (ADR-036):
            // the boundary is on the base, so no subtype can leave it out.
            if let Some(obj) = schema.as_object() {
                validate_parameter_schema_depth(obj, 0)?;
            }
        }

        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false
    }

    fn get_embeddable_content(&self, node: &Node) -> Option<String> {
        let desc = get_namespaced_prop_str(&node.properties, self.type_name(), "description")
            .unwrap_or("");
        let name = &node.content;
        if desc.is_empty() && name.trim().is_empty() {
            None
        } else if desc.is_empty() {
            Some(name.clone())
        } else {
            Some(format!("{}\n\n{}", name, desc))
        }
    }

    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Behavior for `tool-native` nodes: one of NodeSpace's built-in tools.
///
/// Adds the `handler` key to [`ToolNodeBehavior`]'s rules: the stable key of
/// the Rust function the call dispatches to. The node never holds logic.
pub struct ToolNativeNodeBehavior;

impl NodeBehavior for ToolNativeNodeBehavior {
    fn type_name(&self) -> &'static str {
        CoreNodeType::ToolNative.as_str()
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        let handler = get_namespaced_prop_str(&node.properties, self.type_name(), "handler");
        if handler.is_none_or(|key| key.trim().is_empty()) {
            return Err(NodeValidationError::MissingField(
                "tool handler key is required".to_string(),
            ));
        }
        Ok(())
    }

    // `NodeBehaviorRegistry::resolve` answers these from the nearest
    // behaviour in a node's chain, which for a native tool is this one.
    fn supports_markdown(&self) -> bool {
        ToolNodeBehavior.supports_markdown()
    }

    fn get_embeddable_content(&self, node: &Node) -> Option<String> {
        ToolNodeBehavior.get_embeddable_content(node)
    }

    fn get_parent_contribution(&self, node: &Node) -> Option<String> {
        ToolNodeBehavior.get_parent_contribution(node)
    }
}

/// Where a tool comes from, which its subtype says (ADR-086 §12). Every
/// per-subtype rule of the tool family is decided from this one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOrigin {
    /// `tool-native`, or a type extending it: NodeSpace's own code.
    Native,
    /// Any other type extending `tool`: it comes from outside.
    External,
}

impl ToolOrigin {
    /// The origin of a tool whose type has `chain` (nearest scope first), or
    /// `None` when the type is not a tool.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::behaviors::ToolOrigin;
    ///
    /// assert_eq!(ToolOrigin::of(&["tool-native", "tool"]), Some(ToolOrigin::Native));
    /// assert_eq!(ToolOrigin::of(&["tool-remote", "tool"]), Some(ToolOrigin::External));
    /// assert_eq!(ToolOrigin::of(&["text"]), None);
    /// ```
    pub fn of<S: AsRef<str>>(chain: &[S]) -> Option<Self> {
        let core = CoreNodeType::nearest(chain)?;
        if core.is_a(CoreNodeType::ToolNative) {
            Some(Self::Native)
        } else if core.is_a(CoreNodeType::Tool) {
            Some(Self::External)
        } else {
            None
        }
    }

    /// The trust gate: whether a tool of this origin may be offered to the
    /// model. A native tool always is. Every other tool is offered only when
    /// `enabled`, the base field every tool carries.
    pub fn is_offered(self, enabled: bool) -> bool {
        match self {
            Self::Native => true,
            Self::External => enabled,
        }
    }
}

/// The tool trust gate for a type's `chain`: [`ToolOrigin::is_offered`] for a
/// tool, and never for a type that is not one.
///
/// # Examples
///
/// ```rust
/// use nodespace_core::behaviors::tool_is_offered;
///
/// assert!(tool_is_offered(&["tool-native", "tool"], false));
/// assert!(!tool_is_offered(&["tool-remote", "tool"], false));
/// assert!(tool_is_offered(&["tool-remote", "tool"], true));
/// assert!(!tool_is_offered(&["text"], true));
/// ```
pub fn tool_is_offered<S: AsRef<str>>(chain: &[S], enabled: bool) -> bool {
    ToolOrigin::of(chain).is_some_and(|origin| origin.is_offered(enabled))
}

/// Maximum object-nesting depth allowed in a tool's `parameter_schema`.
///
/// Bounded nesting stops a tool registered from outside from storing a schema
/// that floods the model's context or hides type confusion under deeply
/// nested `additionalProperties`. The guard runs for every tool subtype, the
/// built-in ones included.
///
/// `validate_parameter_schema_depth` increments depth on every object-valued
/// key (structural keys like `properties`/`items` included), so a legitimately
/// shaped tool schema nests deeper than its conceptual field nesting suggests.
/// The deepest valid built-in tool is `create_schema`, whose edge-field path
/// `properties → relationships → items → properties → edgeFields → items →
/// properties → coreValues` reaches depth 8 (`coreValues` there is a leaf
/// description, not a further-nested items/properties pair — unlike a node
/// field's own `coreValues`, an edge field's is documented in prose rather
/// than a nested `{value, label}` item schema, precisely because that nesting
/// would exceed this limit). The limit is 9 to admit that and leave a small
/// margin, while still rejecting pathological or unbounded schemas.
const MAX_SCHEMA_DEPTH: usize = 9;

/// Validate that a parameter schema object does not exceed the depth limit
/// and does not use unbounded `additionalProperties: true`.
fn validate_parameter_schema_depth(
    obj: &serde_json::Map<String, serde_json::Value>,
    depth: usize,
) -> Result<(), NodeValidationError> {
    if depth >= MAX_SCHEMA_DEPTH {
        return Err(NodeValidationError::InvalidProperties(format!(
            "parameter_schema exceeds maximum nesting depth of {}",
            MAX_SCHEMA_DEPTH
        )));
    }

    // Reject unbounded additionalProperties: true at any depth
    if let Some(additional) = obj.get("additionalProperties") {
        if additional == &serde_json::Value::Bool(true) {
            return Err(NodeValidationError::InvalidProperties(
                "parameter_schema must not use additionalProperties: true".to_string(),
            ));
        }
    }

    // Recurse into nested schema objects (properties, items, etc.)
    for (_, v) in obj.iter() {
        if let Some(child) = v.as_object() {
            validate_parameter_schema_depth(child, depth + 1)?;
        }
    }

    Ok(())
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

    fn supports_markdown(&self) -> bool {
        false // Custom types don't support markdown by default
    }
}

/// Built-in behavior for person nodes
///
/// Person nodes are pure identity — `first_name` and `last_name` (optional)
/// and `email` (optional, format-validated when present). Being this
/// database's owner is not a PersonNode property: it is the `has_role` edge
/// from the person to the `DatabaseSettingsNode` singleton.
///
/// A person can carry child nodes (notes about them). It is not embedded, and
/// neither are those notes: it is a record found by its title.
pub struct PersonNodeBehavior;

impl NodeBehavior for PersonNodeBehavior {
    fn type_name(&self) -> &'static str {
        "person"
    }

    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        if let Some(email) = node
            .properties
            .get("person")
            .and_then(|p| p.get("email"))
            .and_then(|v| v.as_str())
        {
            if !(email.is_empty() || email.contains('@') && email.contains('.')) {
                return Err(NodeValidationError::InvalidProperties(
                    "email must contain '@' and '.'".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false
    }

    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
    }
}

/// Built-in behavior for the database-settings singleton node
///
/// DatabaseSettingsNode is the singleton anchor for database-level configuration
/// and for the owner `has_role` edge (ADR-037): the edge runs PersonNode →
/// DatabaseSettingsNode and marks that person as the local user. Its one field,
/// `required_extensions`, lists the extensions a reader needs (ADR-083 §2).
pub struct DatabaseSettingsNodeBehavior;

impl NodeBehavior for DatabaseSettingsNodeBehavior {
    fn type_name(&self) -> &'static str {
        "database-settings"
    }

    /// Rejects a `required_extensions` that is not a list of strings (null
    /// clears it). The daemon's open guard refuses a database whose list it
    /// cannot read, so a malformed value must not be stored. Checked in the
    /// node's own bucket and at the top level, where a write that does not
    /// normalize flat properties (`bulk_create`) leaves the field.
    fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
        let field = crate::models::core_schemas::REQUIRED_EXTENSIONS_FIELD;
        let values = [
            node.properties
                .get(self.type_name())
                .and_then(|bucket| bucket.get(field)),
            node.properties.get(field),
        ];
        for value in values.into_iter().flatten() {
            let is_list_of_strings = value.is_null()
                || value
                    .as_array()
                    .is_some_and(|items| items.iter().all(serde_json::Value::is_string));
            if !is_list_of_strings {
                return Err(NodeValidationError::InvalidProperties(format!(
                    "{field} must be a list of strings, found {value}"
                )));
            }
        }
        Ok(())
    }

    fn supports_markdown(&self) -> bool {
        false // Settings are not markdown-rendered content
    }

    /// Settings carry no semantic content; they are not embedded for search.
    fn get_embeddable_content(&self, _node: &Node) -> Option<String> {
        None
    }

    /// Settings do not contribute to parent embeddings.
    fn get_parent_contribution(&self, _node: &Node) -> Option<String> {
        None
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
/// (like "invoice", "customer") to work without Rust code.
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
/// accessed concurrently during runtime, making external synchronization unnecessary.
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
/// // Person nodes also validate via registered behavior
/// let person_node = Node::new(
///     "person".to_string(),
///     "Alice".to_string(),
///     json!({"person": {"first_name": "Alice", "last_name": "Example", "email": "alice@example.com"}}),
/// );
/// assert!(registry.validate_node(&person_node).is_ok());
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

        // One behaviour per core type (ADR-086 §3). The registry tests hold
        // this list to `CoreNodeType::ALL`.
        registry.register_core(Arc::new(TextNodeBehavior));
        registry.register_core(Arc::new(HeaderNodeBehavior));
        registry.register_core(Arc::new(TaskNodeBehavior));
        registry.register_core(Arc::new(ProjectNodeBehavior));
        registry.register_core(Arc::new(CodeBlockNodeBehavior));
        registry.register_core(Arc::new(QuoteBlockNodeBehavior));
        registry.register_core(Arc::new(OrderedListNodeBehavior));
        registry.register_core(Arc::new(CheckboxNodeBehavior));
        registry.register_core(Arc::new(DateNodeBehavior));
        registry.register_core(Arc::new(SchemaNodeBehavior));
        registry.register_core(Arc::new(QueryNodeBehavior));
        registry.register_core(Arc::new(CollectionNodeBehavior));
        registry.register_core(Arc::new(HorizontalLineNodeBehavior));
        registry.register_core(Arc::new(TableNodeBehavior));
        registry.register_core(Arc::new(AiChatNodeBehavior));
        registry.register_core(Arc::new(AiChatNativeNodeBehavior));
        registry.register_core(Arc::new(AiChatPtyNodeBehavior));
        registry.register_core(Arc::new(AiChatMessageNodeBehavior));
        registry.register_core(Arc::new(AgentGuidanceNodeBehavior));
        registry.register_core(Arc::new(SkillNodeBehavior));
        registry.register_core(Arc::new(ToolNodeBehavior));
        registry.register_core(Arc::new(ToolNativeNodeBehavior));
        registry.register_core(Arc::new(PlayNodeBehavior));
        registry.register_core(Arc::new(PersonNodeBehavior));
        registry.register_core(Arc::new(DatabaseSettingsNodeBehavior));

        registry
    }

    /// Register a built-in behaviour for a core type.
    fn register_core(&mut self, behavior: Arc<dyn NodeBehavior>) {
        let type_name = behavior.type_name().to_string();
        debug_assert!(
            CoreNodeType::from_id(&type_name).is_some(),
            "'{type_name}' is not a core type"
        );
        self.behaviors.insert(type_name, behavior);
    }

    /// Registers a behaviour for a type outside the core registry: a subtype
    /// another build adds on top of a core type.
    ///
    /// The behaviour's `type_name()` is the key. It **adds** to the rules of
    /// the types it extends: [`Self::validate_node`] runs every behaviour in a
    /// node's chain, base first, so a subtype can reject more and never less.
    ///
    /// # Errors
    ///
    /// A behaviour for a core type is refused, and so is a second behaviour
    /// for a type that already has one. Another build may add types; it
    /// cannot replace the rules of an existing one (ADR-086 §5).
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::behaviors::{CustomNodeBehavior, NodeBehaviorRegistry, TextNodeBehavior};
    /// use std::sync::Arc;
    ///
    /// let mut registry = NodeBehaviorRegistry::new();
    /// assert!(registry.register(Arc::new(CustomNodeBehavior::new("invoice"))).is_ok());
    /// // `text` is a core type: its behaviour cannot be replaced.
    /// assert!(registry.register(Arc::new(TextNodeBehavior)).is_err());
    /// ```
    pub fn register(
        &mut self,
        behavior: Arc<dyn NodeBehavior>,
    ) -> Result<(), BehaviorRegistrationError> {
        let type_name = behavior.type_name().to_string();
        if CoreNodeType::from_id(&type_name).is_some() {
            return Err(BehaviorRegistrationError::CoreType(type_name));
        }
        if self.behaviors.contains_key(&type_name) {
            return Err(BehaviorRegistrationError::AlreadyRegistered(type_name));
        }
        self.behaviors.insert(type_name, behavior);
        Ok(())
    }

    /// The behaviour registered for exactly `node_type`.
    ///
    /// `None` for a type with no behaviour of its own, which includes every
    /// user-defined subtype of a core type. To apply a type's rules to a node,
    /// resolve the node's chain and use [`Self::resolve`] or
    /// [`Self::validate_node`] instead.
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
    /// assert!(registry.get_all_types().len() >= 3); // At least text, task, date
    /// ```
    pub fn get_all_types(&self) -> Vec<String> {
        self.behaviors.keys().cloned().collect()
    }

    /// Every behaviour that applies to a node whose type has `chain` (nearest
    /// scope first, as `SqliteStore::type_chain` returns it), ordered base
    /// first: `[task behaviour, issue behaviour]` for `["issue", "task"]`.
    pub fn for_chain<S: AsRef<str>>(&self, chain: &[S]) -> Vec<Arc<dyn NodeBehavior>> {
        chain
            .iter()
            .rev()
            .filter_map(|node_type| self.get(node_type.as_ref()))
            .collect()
    }

    /// The behaviour that decides a node's embedding, title and content
    /// rules: the nearest one registered in `chain`, so a type with no
    /// behaviour of its own behaves as the type it extends. A chain with no
    /// registered behaviour gets the schema-defined fallback.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use nodespace_core::behaviors::NodeBehaviorRegistry;
    ///
    /// let registry = NodeBehaviorRegistry::new();
    /// // A user's `issue extends task` is a task.
    /// assert_eq!(registry.resolve(&["issue", "task"]).type_name(), "task");
    /// ```
    pub fn resolve<S: AsRef<str>>(&self, chain: &[S]) -> Arc<dyn NodeBehavior> {
        chain
            .iter()
            .find_map(|node_type| self.get(node_type.as_ref()))
            .unwrap_or_else(|| {
                let own = chain.first().map(|t| t.as_ref()).unwrap_or_default();
                Arc::new(CustomNodeBehavior::new(own))
            })
    }

    /// Validates a node against every behaviour in its type's `chain`
    /// (nearest scope first), base first.
    ///
    /// Composition, not replacement: a subtype's behaviour runs after its
    /// ancestors' and can only add rejections. A chain with no registered
    /// behaviour is a schema-defined type and gets the minimal fallback.
    ///
    /// # Errors
    ///
    /// The first validation error any behaviour in the chain returns.
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
    /// let node = Node::new("text".to_string(), "Hello".to_string(), json!({}));
    /// assert!(registry.validate_node(&node, &["text"]).is_ok());
    ///
    /// // A subtype of `collection` keeps the collection's naming rule.
    /// let team = Node::new("team".to_string(), "a:b".to_string(), json!({}));
    /// assert!(registry.validate_node(&team, &["team", "collection"]).is_err());
    /// ```
    pub fn validate_node<S: AsRef<str>>(
        &self,
        node: &Node,
        chain: &[S],
    ) -> Result<(), NodeValidationError> {
        let behaviors = self.for_chain(chain);
        if behaviors.is_empty() {
            return CustomNodeBehavior::new(&node.node_type).validate(node);
        }
        for behavior in behaviors {
            behavior.validate(node)?;
        }
        Ok(())
    }
}

/// Why a behaviour could not be registered.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum BehaviorRegistrationError {
    /// The type is in the core registry: its rules are fixed.
    #[error("'{0}' is a core type; its behavior cannot be replaced")]
    CoreType(String),
    /// The type already has a behaviour.
    #[error("a behavior is already registered for '{0}'")]
    AlreadyRegistered(String),
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

        // Blank text nodes are now allowed (frontend manages persistence)
        let mut empty_node = valid_node.clone();
        empty_node.content = "".to_string();
        assert!(behavior.validate(&empty_node).is_ok());

        // Whitespace-only content is also allowed
        let mut whitespace_node = valid_node.clone();
        whitespace_node.content = "   ".to_string();
        assert!(behavior.validate(&whitespace_node).is_ok());
    }

    #[test]
    fn test_text_node_unicode_whitespace_validation() {
        let behavior = TextNodeBehavior;
        let base_node = Node::new("text".to_string(), "Valid".to_string(), json!({}));

        // All whitespace (including Unicode) is now allowed
        // Backend no longer validates content - frontend manages blank node persistence

        // Zero-width space (U+200B) - now allowed
        let mut node = base_node.clone();
        node.content = "\u{200B}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Zero-width space should be allowed"
        );

        // Zero-width non-joiner (U+200C) - now allowed
        node.content = "\u{200C}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Zero-width non-joiner should be allowed"
        );

        // Zero-width joiner (U+200D) - now allowed
        node.content = "\u{200D}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Zero-width joiner should be allowed"
        );

        // Non-breaking space (U+00A0) - now allowed
        node.content = "\u{00A0}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Non-breaking space should be allowed"
        );

        // Line separator (U+2028) - now allowed
        node.content = "\u{2028}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Line separator should be allowed"
        );

        // Paragraph separator (U+2029) - now allowed
        node.content = "\u{2029}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Paragraph separator should be allowed"
        );

        // Mixed Unicode whitespace - now allowed
        node.content = "\u{200B}\u{00A0}\u{2028}".to_string();
        assert!(
            behavior.validate(&node).is_ok(),
            "Mixed Unicode whitespace should be allowed"
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
        assert!(behavior.supports_markdown());
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

        // Blank headers are now allowed (e.g., "##" with no content)
        let mut blank_node = valid_node.clone();
        blank_node.content = "".to_string();
        assert!(
            behavior.validate(&blank_node).is_ok(),
            "Blank header nodes should be allowed"
        );

        // Whitespace-only headers are allowed
        let mut whitespace_node = valid_node.clone();
        whitespace_node.content = "   ".to_string();
        assert!(
            behavior.validate(&whitespace_node).is_ok(),
            "Whitespace-only header nodes should be allowed"
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

        // Blank code blocks are now allowed
        let mut blank_node = valid_node.clone();
        blank_node.content = "".to_string();
        assert!(
            behavior.validate(&blank_node).is_ok(),
            "Blank code blocks should be allowed"
        );

        // Whitespace-only code blocks are allowed
        let mut whitespace_node = valid_node.clone();
        whitespace_node.content = "   ".to_string();
        assert!(
            behavior.validate(&whitespace_node).is_ok(),
            "Whitespace-only code blocks should be allowed"
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

        // Blank quote blocks are now allowed (e.g., ">" with no content)
        let mut blank_node = valid_node.clone();
        blank_node.content = "".to_string();
        assert!(
            behavior.validate(&blank_node).is_ok(),
            "Blank quote blocks should be allowed"
        );

        // Quote with just prefix and whitespace
        let mut prefix_only_node = valid_node.clone();
        prefix_only_node.content = ">".to_string();
        assert!(
            behavior.validate(&prefix_only_node).is_ok(),
            "Quote blocks with just '>' should be allowed"
        );

        // Quote with prefix and space
        let mut prefix_space_node = valid_node.clone();
        prefix_space_node.content = "> ".to_string();
        assert!(
            behavior.validate(&prefix_space_node).is_ok(),
            "Quote blocks with just '> ' should be allowed"
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

        // Blank ordered lists are now allowed (consistent with headers, quotes, etc.)
        let mut blank_node = valid_node.clone();
        blank_node.content = "".to_string();
        assert!(
            behavior.validate(&blank_node).is_ok(),
            "Blank ordered list nodes should be allowed"
        );

        // Ordered list with just prefix
        let mut prefix_only_node = valid_node.clone();
        prefix_only_node.content = "1. ".to_string();
        assert!(
            behavior.validate(&prefix_only_node).is_ok(),
            "Ordered lists with just '1. ' should be allowed"
        );

        // Whitespace-only ordered lists are allowed
        let mut whitespace_node = valid_node.clone();
        whitespace_node.content = "   ".to_string();
        assert!(
            behavior.validate(&whitespace_node).is_ok(),
            "Whitespace-only ordered list nodes should be allowed"
        );
    }

    #[test]
    fn test_horizontal_line_behavior_validation() {
        let behavior = HorizontalLineNodeBehavior;

        let valid_node = Node::new("horizontal-line".to_string(), "---".to_string(), json!({}));
        assert!(behavior.validate(&valid_node).is_ok());

        // Empty content is also valid
        let empty_node = Node::new("horizontal-line".to_string(), "".to_string(), json!({}));
        assert!(behavior.validate(&empty_node).is_ok());
    }

    #[test]
    fn test_horizontal_line_behavior_capabilities() {
        let behavior = HorizontalLineNodeBehavior;
        assert_eq!(behavior.type_name(), "horizontal-line");
        assert!(!behavior.supports_markdown());
    }

    #[test]
    fn test_horizontal_line_behavior_embeddings() {
        let behavior = HorizontalLineNodeBehavior;
        let node = Node::new("horizontal-line".to_string(), "---".to_string(), json!({}));
        assert!(behavior.get_embeddable_content(&node).is_none());
        assert!(behavior.get_parent_contribution(&node).is_none());
    }

    #[test]
    fn test_table_behavior_validation() {
        let behavior = TableNodeBehavior;

        let valid_node = Node::new(
            "table".to_string(),
            "| A | B |\n| --- | --- |\n| 1 | 2 |".to_string(),
            json!({}),
        );
        assert!(behavior.validate(&valid_node).is_ok());

        // Empty content is also valid
        let empty_node = Node::new("table".to_string(), "".to_string(), json!({}));
        assert!(behavior.validate(&empty_node).is_ok());
    }

    #[test]
    fn test_table_behavior_capabilities() {
        let behavior = TableNodeBehavior;
        assert_eq!(behavior.type_name(), "table");
        assert!(!behavior.supports_markdown());
    }

    #[test]
    fn test_table_behavior_embeddings() {
        let behavior = TableNodeBehavior;

        // Table with content contributes to embeddings
        let node = Node::new("table".to_string(), "| A | B |".to_string(), json!({}));
        assert!(behavior.get_embeddable_content(&node).is_some());
        assert!(behavior.get_parent_contribution(&node).is_some());

        // Empty table doesn't contribute
        let empty = Node::new("table".to_string(), "".to_string(), json!({}));
        assert!(behavior.get_embeddable_content(&empty).is_none());
        assert!(behavior.get_parent_contribution(&empty).is_none());
    }

    #[test]
    fn test_project_node_behavior_valid() {
        let behavior = ProjectNodeBehavior;
        assert_eq!(behavior.type_name(), "project");
        assert!(behavior.supports_markdown());

        // Full project: name + all typed fields, valid date range.
        let full = Node::new(
            "project".to_string(),
            "Launch v1".to_string(),
            json!({
                "project": {
                    "status": "active",
                    "priority": "high",
                    "start_date": "2026-01-01",
                    "end_date": "2026-06-30"
                }
            }),
        );
        assert!(behavior.validate(&full).is_ok());

        // Minimal project: name only (no properties).
        let minimal = Node::new(
            "project".to_string(),
            "Untitled project".to_string(),
            json!({}),
        );
        assert!(behavior.validate(&minimal).is_ok());

        // Equal start/end dates are allowed (start <= end).
        let same_day = Node::new(
            "project".to_string(),
            "One-day sprint".to_string(),
            json!({ "project": { "start_date": "2026-03-03", "end_date": "2026-03-03" } }),
        );
        assert!(behavior.validate(&same_day).is_ok());
    }

    #[test]
    fn test_project_node_behavior_rejects_empty_name() {
        let behavior = ProjectNodeBehavior;
        let node = Node::new(
            "project".to_string(),
            "   ".to_string(),
            json!({ "project": { "status": "planning" } }),
        );
        assert!(matches!(
            behavior.validate(&node),
            Err(NodeValidationError::MissingField(_))
        ));
    }

    #[test]
    fn test_project_node_behavior_rejects_non_string_fields() {
        let behavior = ProjectNodeBehavior;
        for bad in [
            json!({ "project": { "status": 3 } }),
            json!({ "project": { "priority": true } }),
            json!({ "project": { "start_date": 20260101 } }),
        ] {
            let node = Node::new("project".to_string(), "P".to_string(), bad);
            assert!(matches!(
                behavior.validate(&node),
                Err(NodeValidationError::InvalidProperties(_))
            ));
        }
    }

    #[test]
    fn test_project_node_behavior_rejects_inverted_date_range() {
        let behavior = ProjectNodeBehavior;
        let node = Node::new(
            "project".to_string(),
            "Time-travel project".to_string(),
            json!({ "project": { "start_date": "2026-06-30", "end_date": "2026-01-01" } }),
        );
        assert!(matches!(
            behavior.validate(&node),
            Err(NodeValidationError::InvalidProperties(_))
        ));

        // Non-ISO dates skip the range check (spec: enforce only when both parse).
        let non_iso = Node::new(
            "project".to_string(),
            "Loose dates".to_string(),
            json!({ "project": { "start_date": "June 2026", "end_date": "Jan 2026" } }),
        );
        assert!(behavior.validate(&non_iso).is_ok());
    }

    #[test]
    fn test_project_node_behavior_falls_back_to_flat_properties() {
        // A node predating hoisting, or constructed directly, has its typed
        // fields flat rather than under properties.project.*. Before the
        // fallback this silently skipped validation entirely rather than
        // checking the flat data — a bad flat `status` would pass.
        let behavior = ProjectNodeBehavior;

        let flat_invalid = Node::new(
            "project".to_string(),
            "Flat project".to_string(),
            json!({ "status": 3 }),
        );
        assert!(matches!(
            behavior.validate(&flat_invalid),
            Err(NodeValidationError::InvalidProperties(_))
        ));

        let flat_valid = Node::new(
            "project".to_string(),
            "Flat project".to_string(),
            json!({ "status": "active", "start_date": "2026-01-01" }),
        );
        assert!(behavior.validate(&flat_valid).is_ok());

        // A `project` key present but null (not an object) must fall back to
        // the flat data rather than being treated as "the namespace exists."
        let null_namespace = Node::new(
            "project".to_string(),
            "Flat project".to_string(),
            json!({ "project": null, "status": 3 }),
        );
        assert!(matches!(
            behavior.validate(&null_namespace),
            Err(NodeValidationError::InvalidProperties(_))
        ));
    }

    #[test]
    fn test_project_node_behavior_is_not_embedded() {
        let behavior = ProjectNodeBehavior;
        let node = Node::new("project".to_string(), "Launch v1".to_string(), json!({}));
        assert_eq!(behavior.get_embeddable_content(&node), None);
        assert_eq!(behavior.get_parent_contribution(&node), None);
    }

    #[test]
    fn test_task_node_behavior_validation() {
        let behavior = TaskNodeBehavior;

        // Valid task with status (flat format — used by the markdown importer)
        // Status values use lowercase format
        let valid_node_flat_format = Node::new(
            "task".to_string(),
            "Implement feature".to_string(),
            json!({"status": "in_progress"}),
        );
        assert!(behavior.validate(&valid_node_flat_format).is_ok());

        // Valid task with status (new nested format)
        let valid_node_new_format = Node::new(
            "task".to_string(),
            "Implement feature".to_string(),
            json!({"task": {"status": "in_progress"}}),
        );
        assert!(behavior.validate(&valid_node_new_format).is_ok());

        // Valid task with all fields (new nested format)
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

        // Priority is a string enum. A non-string priority is not a supported
        // format: the typed conversion reads it as a string, so it is ignored and
        // the task falls back to having no priority rather than failing validation.
        let integer_priority_node = Node::new(
            "task".to_string(),
            "Task with non-string priority".to_string(),
            json!({"task": {"status": "open", "priority": 2}}),
        );
        // Passing validation here does NOT mean an integer priority is accepted —
        // any task-typed node validates. The assertion below is the load-bearing
        // one: the value is dropped, not interpreted.
        assert!(behavior.validate(&integer_priority_node).is_ok());
        let typed = crate::models::node_to_typed_value(integer_priority_node).unwrap();
        assert!(
            typed.get("priority").is_none(),
            "non-string priority should be ignored, not interpreted as a legacy format"
        );

        // Valid: empty content (allowed for tasks - users can add description later)
        let mut empty_content_node = valid_node_new_format.clone();
        empty_content_node.content = String::new();
        assert!(behavior.validate(&empty_content_node).is_ok());

        // The behavior never reads the node's own type: it also runs for every
        // type extending `task`, whose nodes carry their own `node_type`.
        let issue = Node::new(
            "issue".to_string(),
            "A subtype of task".to_string(),
            json!({"task": {"status": "open"}}),
        );
        assert!(behavior.validate(&issue).is_ok());

        // NOTE: Status value validation (e.g., "open" vs custom) is handled by TaskStatus enum.
        // Unknown status values become TaskStatus::User(value) for schema extensibility.

        // Invalid status type (number instead of string) falls back gracefully
        let bad_status_type = Node::new(
            "task".to_string(),
            "Task".to_string(),
            json!({"task": {"status": 123}}), // number instead of string
        );
        // The typed conversion ignores an invalid status and uses the default
        // (open). This is graceful degradation - it passes validation
        assert!(behavior.validate(&bad_status_type).is_ok());

        // Priority is now a string enum (highest, high, medium, low, lowest) with user-extensibility
        // All values pass validation - unknown values become Priority::User(value)
        let priority_highest = Node::new(
            "task".to_string(),
            "Task".to_string(),
            json!({"task": {"priority": "highest"}}),
        );
        assert!(behavior.validate(&priority_highest).is_ok());

        let priority_lowest = Node::new(
            "task".to_string(),
            "Task".to_string(),
            json!({"task": {"priority": "lowest"}}),
        );
        assert!(behavior.validate(&priority_lowest).is_ok());

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
        assert!(!behavior.supports_markdown());
    }

    #[test]
    fn test_type_conversion_preserves_properties() {
        // Core value proposition: Properties should be preserved
        // when converting between node types (e.g., task → text → task)

        let behavior = TaskNodeBehavior;

        // Create a task node with properties in the new nested format
        // Status/priority values use lowercase format
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

        // Valid: content can be different from ID
        let mut custom_content = valid_node.clone();
        custom_content.content = "Custom Daily Notes".to_string();
        assert!(behavior.validate(&custom_content).is_ok());
    }

    #[test]
    fn test_date_node_behavior_capabilities() {
        let behavior = DateNodeBehavior;

        assert_eq!(behavior.type_name(), "date");
        assert!(!behavior.supports_markdown());
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

        // A type outside the core registry can be given a behavior.
        registry
            .register(Arc::new(CustomNodeBehavior::new("invoice")))
            .unwrap();
        let behavior = registry.get("invoice");
        assert!(behavior.is_some());
        assert_eq!(behavior.unwrap().type_name(), "invoice");

        // Unknown type should return None
        assert!(registry.get("unknown").is_none());
    }

    /// Another build may add types; it cannot replace the rules of a core one
    /// (ADR-086 §5), nor register a second behavior for a type that has one.
    #[test]
    fn a_behavior_for_a_core_type_is_refused() {
        let mut registry = NodeBehaviorRegistry::new();
        for core in CoreNodeType::ALL {
            assert_eq!(
                registry.register(Arc::new(CustomNodeBehavior::new(core.as_str()))),
                Err(BehaviorRegistrationError::CoreType(
                    core.as_str().to_string()
                )),
            );
        }
        registry
            .register(Arc::new(CustomNodeBehavior::new("invoice")))
            .unwrap();
        assert_eq!(
            registry.register(Arc::new(CustomNodeBehavior::new("invoice"))),
            Err(BehaviorRegistrationError::AlreadyRegistered(
                "invoice".to_string()
            )),
        );
    }

    /// Every core type has exactly one behavior, and no behavior is
    /// registered for a type outside the registry (ADR-086 §3).
    #[test]
    fn the_built_in_behaviors_are_exactly_the_core_types() {
        let registry = NodeBehaviorRegistry::new();
        let mut registered = registry.get_all_types();
        registered.sort();
        let mut core: Vec<String> = CoreNodeType::ALL
            .iter()
            .map(|t| t.as_str().to_string())
            .collect();
        core.sort();
        assert_eq!(registered, core);
        for core in CoreNodeType::ALL {
            assert_eq!(
                registry.get(core.as_str()).unwrap().type_name(),
                core.as_str()
            );
        }
    }

    /// What the registry records as embedded is what the behavior does.
    #[test]
    fn a_core_type_is_embedded_exactly_when_the_registry_says_so() {
        let registry = NodeBehaviorRegistry::new();
        for core in CoreNodeType::ALL {
            let probe = Node::new(
                core.as_str().to_string(),
                "probe content".to_string(),
                json!({}),
            );
            let embeddable = registry
                .resolve(&[core.as_str()])
                .get_embeddable_content(&probe)
                .is_some();
            assert_eq!(
                embeddable,
                core.participation().embedded,
                "{core}: the behavior and the registry disagree on embedding"
            );
        }
    }

    /// A type with no behavior of its own takes the rules of the type it
    /// extends: validation composes base first, and embedding resolves to the
    /// nearest registered behavior.
    #[test]
    fn a_subtype_is_validated_and_embedded_as_the_type_it_extends() {
        let registry = NodeBehaviorRegistry::new();

        // `team extends collection` keeps the collection's naming rule.
        let team = Node::new("team".to_string(), "a:b".to_string(), json!({}));
        assert!(registry
            .validate_node(&team, &["team", "collection"])
            .is_err());
        assert!(registry.validate_node(&team, &["team"]).is_ok());

        // `issue extends task` is not embedded, as a task is not.
        let issue = Node::new("issue".to_string(), "Fix it".to_string(), json!({}));
        assert!(registry
            .resolve(&["issue", "task"])
            .get_embeddable_content(&issue)
            .is_none());
        assert_eq!(
            registry
                .for_chain(&["bug", "issue", "task"])
                .iter()
                .map(|b| b.type_name())
                .collect::<Vec<_>>(),
            vec!["task"]
        );
    }

    /// A registered subtype behavior adds to its base's rules; it cannot relax
    /// them.
    #[test]
    fn a_subtype_behavior_adds_to_its_bases_rules() {
        struct NamedTeam;
        impl NodeBehavior for NamedTeam {
            fn type_name(&self) -> &'static str {
                "team"
            }
            fn validate(&self, node: &Node) -> Result<(), NodeValidationError> {
                if node.content.starts_with("team-") {
                    Ok(())
                } else {
                    Err(NodeValidationError::InvalidProperties(
                        "a team's name starts with team-".to_string(),
                    ))
                }
            }
            fn supports_markdown(&self) -> bool {
                false
            }
        }

        let mut registry = NodeBehaviorRegistry::new();
        registry.register(Arc::new(NamedTeam)).unwrap();
        let chain = ["team", "collection"];
        let node = |content: &str| Node::new("team".to_string(), content.to_string(), json!({}));

        assert!(registry.validate_node(&node("team-core"), &chain).is_ok());
        // The subtype's own rule rejects.
        assert!(registry.validate_node(&node("core"), &chain).is_err());
        // The base's rule still rejects what the subtype would accept.
        assert!(registry.validate_node(&node("team-a:b"), &chain).is_err());
        assert_eq!(
            registry
                .for_chain(&chain)
                .iter()
                .map(|b| b.type_name())
                .collect::<Vec<_>>(),
            vec!["collection", "team"]
        );
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
        assert!(types.contains(&"query".to_string()));
        assert!(types.contains(&"collection".to_string()));
        assert!(types.contains(&"horizontal-line".to_string()));
        assert!(types.contains(&"table".to_string()));
        assert!(types.contains(&"ai-chat".to_string()));
        assert!(types.contains(&"ai-chat-native".to_string()));
        assert!(types.contains(&"ai-chat-pty".to_string()));
        assert!(types.contains(&"agent-guidance".to_string()));
        assert!(types.contains(&"skill".to_string()));
        assert!(types.contains(&"tool".to_string()));
        assert!(types.contains(&"tool-native".to_string()));
        assert!(types.contains(&"person".to_string()));
        assert!(types.contains(&"database-settings".to_string()));
        assert!(types.contains(&"project".to_string()));
        assert!(types.contains(&"play".to_string()));
        assert!(types.contains(&"checkbox".to_string()));
        assert_eq!(types.len(), CoreNodeType::ALL.len());
    }

    #[test]
    fn test_registry_validate_node() {
        let registry = NodeBehaviorRegistry::new();

        // Valid text node
        let text_node = Node::new("text".to_string(), "Hello".to_string(), json!({}));
        assert!(registry.validate_node(&text_node, &["text"]).is_ok());

        // Valid task node (status uses lowercase format)
        let task_node = Node::new(
            "task".to_string(),
            "Do something".to_string(),
            json!({"status": "open"}),
        );
        assert!(registry.validate_node(&task_node, &["task"]).is_ok());

        // Unknown node type now uses CustomNodeBehavior fallback and passes basic validation
        let unknown_node = Node::new("unknown".to_string(), "Content".to_string(), json!({}));
        let result = registry.validate_node(&unknown_node, &["unknown"]);
        assert!(
            result.is_ok(),
            "Unknown node types should use CustomNodeBehavior fallback"
        );

        // But invalid properties (non-object) still fail
        let mut bad_properties_node =
            Node::new("unknown".to_string(), "Content".to_string(), json!({}));
        bad_properties_node.properties = serde_json::json!("not an object");
        let result = registry.validate_node(&bad_properties_node, &["unknown"]);
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
                assert!(registry_clone.validate_node(&node, &["text"]).is_ok());
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
        assert!(behavior.supports_markdown());

        let node = Node::new("text".to_string(), "Test".to_string(), json!({}));
        assert!(behavior.validate(&node).is_ok());
    }

    // =========================================================================
    // Two-Level Embeddability Tests
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
        // Status uses lowercase format
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

        // Task node - not embeddable (status uses lowercase format)
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
    fn test_schema_node_behavior_validate_schema_node() {
        let behavior = SchemaNodeBehavior;

        // A valid schema row
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
        let schema = crate::models::schema_node::from_storage(valid_node, Vec::new()).unwrap();
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
        let empty_schema =
            crate::models::schema_node::from_storage(empty_content_node, Vec::new()).unwrap();
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
        let whitespace_schema =
            crate::models::schema_node::from_storage(whitespace_node, Vec::new()).unwrap();
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
    fn test_schema_field_name_accepts_bare_and_namespaced() {
        for name in [
            "capacity",
            "contact_email",
            "field1",
            "custom:capacity",
            "custom:contact_email",
            "org:cost_center",
            "plugin:external_id",
        ] {
            assert!(
                validate_schema_field_name(name).is_ok(),
                "'{}' should be a valid field name",
                name
            );
        }
    }

    #[test]
    fn test_schema_field_name_rejects_malformed_names() {
        for name in [
            "",                // empty
            "custom:",         // empty bare name
            ":capacity",       // empty namespace
            "custom:a:b",      // more than one prefix
            "custom capacity", // space
            "custom.capacity", // dot is not a namespace separator
            "custom:has space",
        ] {
            assert!(
                validate_schema_field_name(name).is_err(),
                "'{}' should be rejected",
                name
            );
        }
    }

    #[test]
    fn test_schema_node_accepts_namespaced_field_names() {
        let behavior = SchemaNodeBehavior;

        // Namespaced field names are what the description-inference path produces,
        // so validation must accept them.
        let namespaced_node = Node::new(
            "schema".to_string(),
            "venue".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [
                    {
                        "name": "custom:contact_email",
                        "type": "text",
                        "protection": "user",
                        "indexed": false
                    }
                ]
            }),
        );

        assert!(
            behavior.validate(&namespaced_node).is_ok(),
            "Namespaced field names should pass validation"
        );
    }

    #[test]
    fn test_schema_node_validates_namespaced_names_at_nesting_depth() {
        let behavior = SchemaNodeBehavior;

        // Name validation recurses through `fields` and `item_fields`, so the
        // namespace rule must hold at depth, not just on top-level fields.
        let nested = Node::new(
            "schema".to_string(),
            "venue".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [
                    {
                        "name": "custom:address",
                        "type": "object",
                        "protection": "user",
                        "indexed": false,
                        "fields": [
                            {
                                "name": "custom:postal_code",
                                "type": "text",
                                "protection": "user",
                                "indexed": false
                            }
                        ]
                    },
                    {
                        "name": "custom:sessions",
                        "type": "array",
                        "protection": "user",
                        "indexed": false,
                        "item_fields": [
                            {
                                "name": "custom:room",
                                "type": "text",
                                "protection": "user",
                                "indexed": false
                            }
                        ]
                    }
                ]
            }),
        );

        assert!(
            behavior.validate(&nested).is_ok(),
            "Namespaced nested and item field names should pass validation"
        );

        // A malformed nested name must still be rejected at depth.
        let bad_nested = Node::new(
            "schema".to_string(),
            "venue".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "fields": [
                    {
                        "name": "custom:address",
                        "type": "object",
                        "protection": "user",
                        "indexed": false,
                        "fields": [
                            {
                                "name": "custom:a:b",
                                "type": "text",
                                "protection": "user",
                                "indexed": false
                            }
                        ]
                    }
                ]
            }),
        );

        assert!(
            behavior.validate(&bad_nested).is_err(),
            "Malformed nested field names should still be rejected"
        );
    }

    #[test]
    fn test_schema_node_title_template_resolves_namespaced_field() {
        let behavior = SchemaNodeBehavior;

        // Templates are checked against the STORED field names, so a schema
        // created from a description must reference the namespaced form.
        let resolved = Node::new(
            "schema".to_string(),
            "venue".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "titleTemplate": "{custom:contact_email}",
                "fields": [
                    {
                        "name": "custom:contact_email",
                        "type": "text",
                        "protection": "user",
                        "indexed": false
                    }
                ]
            }),
        );

        assert!(
            behavior.validate(&resolved).is_ok(),
            "titleTemplate referencing a namespaced field should validate"
        );

        // The bare name is a different key and must not resolve.
        let unresolved = Node::new(
            "schema".to_string(),
            "venue".to_string(),
            json!({
                "isCore": false,
                "version": 1,
                "titleTemplate": "{contact_email}",
                "fields": [
                    {
                        "name": "custom:contact_email",
                        "type": "text",
                        "protection": "user",
                        "indexed": false
                    }
                ]
            }),
        );

        assert!(
            behavior.validate(&unresolved).is_err(),
            "Bare name must not resolve against a namespaced field"
        );
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
                            { "value": "completed", "label": "Completed" }
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
    // title_template Validation Tests
    // =========================================================================

    #[test]
    fn test_title_template_valid() {
        let behavior = SchemaNodeBehavior;

        let node = Node::new(
            "schema".to_string(),
            "Customer".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": "",
                "titleTemplate": "{first_name} {last_name}",
                "fields": [
                    {"name": "first_name", "type": "text", "protection": "user", "indexed": false},
                    {"name": "last_name", "type": "text", "protection": "user", "indexed": false}
                ],
                "relationships": []
            }),
        );
        let schema = crate::models::schema_node::from_storage(node, Vec::new()).unwrap();
        assert!(
            behavior.validate_schema_node(&schema).is_ok(),
            "Valid title_template should pass validation"
        );
    }

    #[test]
    fn test_title_template_unclosed_brace_rejected() {
        let behavior = SchemaNodeBehavior;

        let node = Node::new(
            "schema".to_string(),
            "Customer".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": "",
                "titleTemplate": "{first_name",
                "fields": [],
                "relationships": []
            }),
        );
        let schema = crate::models::schema_node::from_storage(node, Vec::new()).unwrap();
        let result = behavior.validate_schema_node(&schema);
        assert!(result.is_err(), "Unclosed brace should fail validation");
        assert!(matches!(
            result,
            Err(NodeValidationError::InvalidProperties(ref msg))
                if msg.contains("unclosed")
        ));
    }

    #[test]
    fn test_title_template_empty_placeholder_rejected() {
        let behavior = SchemaNodeBehavior;

        let node = Node::new(
            "schema".to_string(),
            "Customer".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": "",
                "titleTemplate": "{} {last_name}",
                "fields": [],
                "relationships": []
            }),
        );
        let schema = crate::models::schema_node::from_storage(node, Vec::new()).unwrap();
        let result = behavior.validate_schema_node(&schema);
        assert!(result.is_err(), "Empty placeholder should fail validation");
        assert!(matches!(
            result,
            Err(NodeValidationError::InvalidProperties(ref msg))
                if msg.contains("empty")
        ));
    }

    #[test]
    fn test_title_template_none_is_valid() {
        let behavior = SchemaNodeBehavior;

        // No title_template field at all — should pass
        let node = Node::new(
            "schema".to_string(),
            "Widget".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": "",
                "fields": [],
                "relationships": []
            }),
        );
        let schema = crate::models::schema_node::from_storage(node, Vec::new()).unwrap();
        assert!(
            behavior.validate_schema_node(&schema).is_ok(),
            "Schema without title_template should pass validation"
        );
    }

    #[test]
    fn test_title_template_undefined_field_rejected() {
        let behavior = SchemaNodeBehavior;

        // title_template references a field "nonexistent" that is not in schema.fields
        let node = Node::new(
            "schema".to_string(),
            "Customer".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": "Customer schema",
                "fields": [
                    {
                        "name": "first_name",
                        "type": "text",
                        "protection": "user",
                        "indexed": false
                    }
                ],
                "relationships": [],
                "titleTemplate": "{nonexistent}"
            }),
        );
        let schema = crate::models::schema_node::from_storage(node, Vec::new()).unwrap();
        let result = behavior.validate_schema_node(&schema);
        assert!(
            result.is_err(),
            "Undefined field in title_template should fail validation"
        );
        assert!(matches!(
            result,
            Err(NodeValidationError::InvalidProperties(ref msg))
                if msg.contains("undefined field") && msg.contains("nonexistent")
        ));
    }

    #[test]
    fn test_title_template_defined_field_accepted() {
        let behavior = SchemaNodeBehavior;

        // title_template references fields that exist in schema.fields
        let node = Node::new(
            "schema".to_string(),
            "Customer".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": "Customer schema",
                "fields": [
                    {
                        "name": "first_name",
                        "type": "text",
                        "protection": "user",
                        "indexed": false
                    },
                    {
                        "name": "last_name",
                        "type": "text",
                        "protection": "user",
                        "indexed": false
                    }
                ],
                "relationships": [],
                "titleTemplate": "{first_name} {last_name}"
            }),
        );
        let schema = crate::models::schema_node::from_storage(node, Vec::new()).unwrap();
        assert!(
            behavior.validate_schema_node(&schema).is_ok(),
            "title_template referencing defined fields should pass validation"
        );
    }

    #[test]
    fn test_properties_header_summary_template_valid() {
        let behavior = SchemaNodeBehavior;

        // propertiesHeaderSummaryTemplate referencing defined fields should pass
        let node = Node::new(
            "schema".to_string(),
            "Customer".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": "Customer schema",
                "fields": [
                    {"name": "status", "type": "enum", "protection": "user", "indexed": false, "coreValues": [{"value": "active", "label": "Active"}, {"value": "inactive", "label": "Inactive"}]},
                    {"name": "company", "type": "text", "protection": "user", "indexed": false}
                ],
                "relationships": [],
                "propertiesHeaderSummaryTemplate": "{status} · {company}"
            }),
        );
        let schema = crate::models::schema_node::from_storage(node, Vec::new()).unwrap();
        assert!(
            behavior.validate_schema_node(&schema).is_ok(),
            "Valid propertiesHeaderSummaryTemplate should pass validation"
        );
    }

    #[test]
    fn test_properties_header_summary_template_undefined_field_rejected() {
        let behavior = SchemaNodeBehavior;

        // propertiesHeaderSummaryTemplate references a field not in schema.fields
        let node = Node::new(
            "schema".to_string(),
            "Customer".to_string(),
            json!({
                "isCore": false,
                "schemaVersion": 1,
                "description": "Customer schema",
                "fields": [
                    {"name": "status", "type": "enum", "protection": "user", "indexed": false, "coreValues": [{"value": "active", "label": "Active"}, {"value": "inactive", "label": "Inactive"}]}
                ],
                "relationships": [],
                "propertiesHeaderSummaryTemplate": "{status} · {nonexistent}"
            }),
        );
        let schema = crate::models::schema_node::from_storage(node, Vec::new()).unwrap();
        let result = behavior.validate_schema_node(&schema);
        assert!(
            result.is_err(),
            "propertiesHeaderSummaryTemplate referencing undefined field should fail"
        );
        assert!(matches!(
            result,
            Err(NodeValidationError::InvalidProperties(ref msg))
                if msg.contains("nonexistent")
        ));
    }

    #[test]
    fn test_query_node_behavior_validation() {
        let behavior = QueryNodeBehavior;

        // Valid query node with description
        let valid_node = Node::new(
            "query".to_string(),
            "All open high-priority tasks".to_string(),
            json!({}),
        );
        assert!(behavior.validate(&valid_node).is_ok());

        // Empty content is allowed (query definition is in properties)
        let empty_node = Node::new("query".to_string(), "".to_string(), json!({}));
        assert!(behavior.validate(&empty_node).is_ok());

        // Whitespace-only content is allowed
        let whitespace_node = Node::new("query".to_string(), "   ".to_string(), json!({}));
        assert!(behavior.validate(&whitespace_node).is_ok());
    }

    #[test]
    fn test_query_node_behavior_capabilities() {
        let behavior = QueryNodeBehavior;

        assert_eq!(behavior.type_name(), "query");
        assert!(!behavior.supports_markdown());
    }

    #[test]
    fn test_query_node_embedding_behavior() {
        let behavior = QueryNodeBehavior;
        let node = Node::new("query".to_string(), "Find all tasks".to_string(), json!({}));

        // Query nodes should not be embedded (operational, not semantic content)
        assert!(behavior.get_embeddable_content(&node).is_none());
        assert!(behavior.get_parent_contribution(&node).is_none());
    }

    // =========================================================================
    // CollectionNodeBehavior Tests
    // =========================================================================

    #[test]
    fn test_collection_node_behavior_validation() {
        let behavior = CollectionNodeBehavior;

        // Valid collection node with name
        let valid_node = Node::new(
            "collection".to_string(),
            "Engineering".to_string(),
            json!({}),
        );
        assert!(behavior.validate(&valid_node).is_ok());

        // Valid collection with spaces in name
        let spaced_name = Node::new(
            "collection".to_string(),
            "Human Resources".to_string(),
            json!({}),
        );
        assert!(behavior.validate(&spaced_name).is_ok());
    }

    #[test]
    fn test_collection_node_empty_name_rejected() {
        let behavior = CollectionNodeBehavior;

        // Empty content should be rejected
        let empty_node = Node::new("collection".to_string(), "".to_string(), json!({}));
        let result = behavior.validate(&empty_node);
        assert!(result.is_err());
        assert!(matches!(result, Err(NodeValidationError::MissingField(_))));

        // Whitespace-only content should be rejected
        let whitespace_node = Node::new("collection".to_string(), "   ".to_string(), json!({}));
        let result = behavior.validate(&whitespace_node);
        assert!(result.is_err());
        assert!(matches!(result, Err(NodeValidationError::MissingField(_))));
    }

    #[test]
    fn test_collection_node_colon_in_name_rejected() {
        let behavior = CollectionNodeBehavior;

        // Name with colon should be rejected (colon is path delimiter)
        let colon_node = Node::new("collection".to_string(), "hr:policy".to_string(), json!({}));
        let result = behavior.validate(&colon_node);
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(NodeValidationError::InvalidProperties(_))
        ));
    }

    #[test]
    fn test_collection_node_behavior_capabilities() {
        let behavior = CollectionNodeBehavior;

        assert_eq!(behavior.type_name(), "collection");
        assert!(!behavior.supports_markdown()); // Collection names are plain text
    }

    #[test]
    fn test_collection_node_embedding_behavior() {
        let behavior = CollectionNodeBehavior;
        let node = Node::new(
            "collection".to_string(),
            "Engineering".to_string(),
            json!({}),
        );

        // Collection nodes should not be embedded (organizational containers)
        assert!(behavior.get_embeddable_content(&node).is_none());
        assert!(behavior.get_parent_contribution(&node).is_none());
    }

    // ---- AI Chat Node Behavior Tests ----

    /// The behaviours a chat of `node_type` is validated by, base first.
    fn chat_chain(node_type: &str) -> [&str; 2] {
        [node_type, "ai-chat"]
    }

    #[test]
    fn test_ai_chat_node_behavior_validation() {
        let registry = NodeBehaviorRegistry::new();

        let native = Node::new(
            "ai-chat-native".to_string(),
            "Implement webhook handler".to_string(),
            json!({
                "agent": "nodespace",
                "provider": "native",
                "model": "gemma-4-e4b-q4km",
                "turn_status": "idle"
            }),
        );
        assert!(registry
            .validate_node(&native, &chat_chain("ai-chat-native"))
            .is_ok());

        let pty = Node::new(
            "ai-chat-pty".to_string(),
            "Untitled".to_string(),
            json!({ "agent": "claude-code", "session_status": "active" }),
        );
        assert!(registry
            .validate_node(&pty, &chat_chain("ai-chat-pty"))
            .is_ok());
    }

    /// A chat must carry a title, whichever subtype it is: the rule is the
    /// base's, and a subtype's behaviour adds to it. Omitting one is an error
    /// rather than a request for automatic titling — the titler claims only
    /// the explicit `"Untitled"` sentinel, so a client that writes nothing
    /// would otherwise be silently opted into desktop-UI titling behaviour.
    #[test]
    fn test_every_ai_chat_subtype_rejects_empty_content() {
        let registry = NodeBehaviorRegistry::new();

        for node_type in ["ai-chat-native", "ai-chat-pty"] {
            for blank in ["", "   ", "\t\n", "\u{200B}"] {
                let node = Node::new(node_type.to_string(), blank.to_string(), json!({}));
                let err = registry
                    .validate_node(&node, &chat_chain(node_type))
                    .expect_err("a chat without a title must be rejected");
                assert!(
                    matches!(err, NodeValidationError::MissingField(ref f) if f.contains("content")),
                    "{node_type}: expected a missing-content error, got {err:?}"
                );
                // The message must name the opt-in, so a client hitting this
                // knows what to send instead.
                assert!(
                    format!("{err}").contains("Untitled"),
                    "the error must name the \"Untitled\" opt-in, got {err}"
                );
            }

            // The sentinel itself is a title, and is accepted.
            let opted_in = Node::new(node_type.to_string(), "Untitled".to_string(), json!({}));
            assert!(registry
                .validate_node(&opted_in, &chat_chain(node_type))
                .is_ok());
        }
    }

    /// Only an assistant message records how its turn ended, in either
    /// property shape. A message's text is its content and may be anything.
    #[test]
    fn test_ai_chat_message_outcome_is_an_assistant_messages() {
        let registry = NodeBehaviorRegistry::new();
        let chain = ["ai-chat-message"];
        let message = |properties: serde_json::Value| {
            Node::new("ai-chat-message".to_string(), "hi".to_string(), properties)
        };

        for properties in [
            json!({}),
            json!({ "role": "user" }),
            json!({ "role": "user", "outcome": null }),
            json!({ "role": "assistant", "outcome": "replied", "options": ["a"] }),
            json!({ "ai-chat-message": { "role": "assistant", "outcome": "acted" } }),
            json!({ "role": "system" }),
        ] {
            assert!(
                registry
                    .validate_node(&message(properties.clone()), &chain)
                    .is_ok(),
                "{properties} must be accepted"
            );
        }

        for properties in [
            json!({ "outcome": "replied" }),
            json!({ "role": "user", "outcome": "replied" }),
            json!({ "ai-chat-message": { "role": "system", "outcome": "acted" } }),
        ] {
            let err = registry
                .validate_node(&message(properties.clone()), &chain)
                .expect_err("an outcome off an assistant message must be refused");
            assert!(
                err.to_string().contains("assistant message only"),
                "{properties}: {err}"
            );
        }
    }

    /// A message is never embedded and adds nothing to its chat's embedding.
    #[test]
    fn test_ai_chat_message_is_never_embeddable() {
        let registry = NodeBehaviorRegistry::new();
        let behavior = registry.get("ai-chat-message").expect("registered");
        let node = Node::new(
            "ai-chat-message".to_string(),
            "Help me implement the webhook handler".to_string(),
            json!({ "role": "user" }),
        );
        assert!(!behavior.supports_markdown());
        assert!(behavior.get_embeddable_content(&node).is_none());
        assert!(behavior.get_parent_contribution(&node).is_none());
    }

    /// A chat may hold children, and every subtype inherits the rule.
    #[test]
    fn test_ai_chat_node_capabilities() {
        let registry = NodeBehaviorRegistry::new();
        for node_type in ["ai-chat", "ai-chat-native", "ai-chat-pty"] {
            let behavior = registry.get(node_type).expect("registered");
            assert_eq!(behavior.type_name(), node_type);
            assert!(!behavior.supports_markdown());
        }
    }

    #[test]
    fn test_ai_chat_node_never_embeddable() {
        let registry = NodeBehaviorRegistry::new();

        for node_type in ["ai-chat-native", "ai-chat-pty"] {
            let behavior = registry.resolve(&chat_chain(node_type));

            // Not embeddable, with or without a summary.
            for props in [
                json!({ "summary": "Implemented the webhook handler" }),
                json!({}),
            ] {
                let node = Node::new(
                    node_type.to_string(),
                    "Chat about webhooks".to_string(),
                    props,
                );
                assert!(
                    behavior.get_embeddable_content(&node).is_none(),
                    "{node_type} must not be embeddable"
                );
                assert!(behavior.get_parent_contribution(&node).is_none());
            }
        }
    }

    #[test]
    fn test_the_ai_chat_family_is_registered() {
        let registry = NodeBehaviorRegistry::new();
        for node_type in ["ai-chat", "ai-chat-native", "ai-chat-pty"] {
            let behavior = registry.get(node_type);
            assert!(behavior.is_some(), "{node_type} should be registered");
            assert_eq!(behavior.unwrap().type_name(), node_type);
        }
        // A subtype is validated by the base's behaviour and then its own.
        let chain: Vec<&str> = registry
            .for_chain(&chat_chain("ai-chat-native"))
            .iter()
            .map(|b| b.type_name())
            .collect();
        assert_eq!(chain, ["ai-chat", "ai-chat-native"]);
    }

    // =========================================================================
    // Comprehensive: Every behavior's get_embeddable_content()
    // =========================================================================

    /// Verify the embeddable content decision for every registered behavior type.
    /// Types that return Some(...) are embedded as roots; types that return None are not.
    #[test]
    fn test_all_behaviors_embeddable_content_decision() {
        let registry = NodeBehaviorRegistry::new();

        // --- Types that SHOULD be embeddable (return Some for non-empty content) ---

        // text: primary knowledge content
        let text_node = Node::new("text".to_string(), "Some knowledge".to_string(), json!({}));
        assert!(
            registry
                .get("text")
                .unwrap()
                .get_embeddable_content(&text_node)
                .is_some(),
            "text nodes with content should be embeddable"
        );

        // header: section titles carry semantic value
        let header_node = Node::new(
            "header".to_string(),
            "## Architecture".to_string(),
            json!({"headerLevel": 2}),
        );
        assert!(
            registry
                .get("header")
                .unwrap()
                .get_embeddable_content(&header_node)
                .is_some(),
            "header nodes with content should be embeddable"
        );

        // code-block: code snippets are searchable (uses default trait impl)
        let code_node = Node::new(
            "code-block".to_string(),
            "```rust\nfn main() {}".to_string(),
            json!({"language": "rust"}),
        );
        assert!(
            registry
                .get("code-block")
                .unwrap()
                .get_embeddable_content(&code_node)
                .is_some(),
            "code-block nodes with content should be embeddable"
        );

        // quote-block: quoted text is searchable (uses default trait impl)
        let quote_node = Node::new(
            "quote-block".to_string(),
            "> Important quote".to_string(),
            json!({}),
        );
        assert!(
            registry
                .get("quote-block")
                .unwrap()
                .get_embeddable_content(&quote_node)
                .is_some(),
            "quote-block nodes with content should be embeddable"
        );

        // ordered-list: list content is searchable (uses default trait impl)
        let list_node = Node::new(
            "ordered-list".to_string(),
            "1. First step".to_string(),
            json!({}),
        );
        assert!(
            registry
                .get("ordered-list")
                .unwrap()
                .get_embeddable_content(&list_node)
                .is_some(),
            "ordered-list nodes with content should be embeddable"
        );

        // table: table data is searchable
        let table_node = Node::new(
            "table".to_string(),
            "| A | B |\n| 1 | 2 |".to_string(),
            json!({}),
        );
        assert!(
            registry
                .get("table")
                .unwrap()
                .get_embeddable_content(&table_node)
                .is_some(),
            "table nodes with content should be embeddable"
        );

        // schema: schema name is content (uses default trait impl)
        let schema_node = Node::new(
            "schema".to_string(),
            "task".to_string(),
            json!({"is_core": true, "fields": []}),
        );
        assert!(
            registry
                .get("schema")
                .unwrap()
                .get_embeddable_content(&schema_node)
                .is_some(),
            "schema nodes with content should be embeddable"
        );

        // ai-chat: conversations are deliberately NOT embedded
        let chat_node = Node::new(
            "ai-chat-native".to_string(),
            "Chat about webhooks".to_string(),
            json!({ "summary": "How to implement webhooks" }),
        );
        let chat_content = registry
            .get("ai-chat-native")
            .unwrap()
            .get_embeddable_content(&chat_node);
        assert!(chat_content.is_none(), "ai-chat should never be embeddable");

        // --- Types that should NOT be embeddable (return None) ---

        // task: action items, not semantic knowledge
        let task_node = Node::new(
            "task".to_string(),
            "Buy groceries".to_string(),
            json!({"task": {"status": "open"}}),
        );
        assert!(
            registry
                .get("task")
                .unwrap()
                .get_embeddable_content(&task_node)
                .is_none(),
            "task nodes should NOT be embeddable"
        );

        // date: organizational containers
        let date_node = Node::new_with_id(
            "2025-06-15".to_string(),
            "date".to_string(),
            "2025-06-15".to_string(),
            json!({}),
        );
        assert!(
            registry
                .get("date")
                .unwrap()
                .get_embeddable_content(&date_node)
                .is_none(),
            "date nodes should NOT be embeddable"
        );

        // horizontal-line: decorative, no semantic content
        let hr_node = Node::new("horizontal-line".to_string(), "---".to_string(), json!({}));
        assert!(
            registry
                .get("horizontal-line")
                .unwrap()
                .get_embeddable_content(&hr_node)
                .is_none(),
            "horizontal-line nodes should NOT be embeddable"
        );

        // query: operational/structural, not semantic
        let query_node = Node::new(
            "query".to_string(),
            "Find open tasks".to_string(),
            json!({}),
        );
        assert!(
            registry
                .get("query")
                .unwrap()
                .get_embeddable_content(&query_node)
                .is_none(),
            "query nodes should NOT be embeddable"
        );

        // collection: organizational labels
        let coll_node = Node::new(
            "collection".to_string(),
            "Engineering".to_string(),
            json!({}),
        );
        assert!(
            registry
                .get("collection")
                .unwrap()
                .get_embeddable_content(&coll_node)
                .is_none(),
            "collection nodes should NOT be embeddable"
        );

        // --- ai-chat with no messages also returns None (never embeddable) ---
        let empty_chat = Node::new("ai-chat-native".to_string(), "Empty".to_string(), json!({}));
        assert!(
            registry
                .get("ai-chat-native")
                .unwrap()
                .get_embeddable_content(&empty_chat)
                .is_none(),
            "ai-chat is never embeddable, with or without messages"
        );
    }

    /// Verify that CustomNodeBehavior (fallback for schema-defined types) uses the
    /// default trait implementation for embeddable content (embeddable if non-empty).
    #[test]
    fn test_custom_behavior_uses_default_embeddability() {
        let behavior = CustomNodeBehavior::new("invoice");

        let node = Node::new("invoice".to_string(), "INV-001".to_string(), json!({}));
        assert!(
            behavior.get_embeddable_content(&node).is_some(),
            "Custom type with content should be embeddable (default trait impl)"
        );

        let empty = Node::new("invoice".to_string(), "".to_string(), json!({}));
        assert!(
            behavior.get_embeddable_content(&empty).is_none(),
            "Custom type with empty content should NOT be embeddable"
        );
    }

    // ToolNodeBehavior tests ------------------------------------

    /// A native tool as it is stored: the base's fields in the `tool` bucket,
    /// the handler in its own.
    fn tool_node_with_props(props: serde_json::Value) -> Node {
        Node::new("tool-native".to_string(), "search_nodes".to_string(), props)
    }

    const NATIVE_TOOL_CHAIN: [&str; 2] = ["tool-native", "tool"];

    fn validate_native_tool(node: &Node) -> Result<(), NodeValidationError> {
        NodeBehaviorRegistry::new().validate_node(node, &NATIVE_TOOL_CHAIN)
    }

    #[test]
    fn tool_node_valid_accepts_well_formed_node() {
        let node = tool_node_with_props(json!({
            "tool": {
                "description": "Search nodes by keyword",
                "parameter_schema": {
                    "type": "object",
                    "properties": { "query": { "type": "string" } }
                },
                "enabled": true,
            },
            "tool-native": { "handler": "search_nodes" },
        }));
        assert!(validate_native_tool(&node).is_ok());
    }

    #[test]
    fn tool_node_rejects_empty_handler() {
        let node = tool_node_with_props(json!({
            "tool": { "description": "A tool" },
            "tool-native": { "handler": "" },
        }));
        let err = validate_native_tool(&node).unwrap_err();
        assert!(
            format!("{}", err).contains("handler"),
            "Error should mention handler: {}",
            err
        );
    }

    #[test]
    fn tool_node_rejects_missing_handler() {
        let node = tool_node_with_props(json!({ "tool": { "description": "A tool" } }));
        assert!(validate_native_tool(&node).is_err());
        // The handler is the native subtype's rule: the base asks for none.
        assert!(ToolNodeBehavior.validate(&node).is_ok());
    }

    /// A handler left in the base's bucket is not the native tool's handler.
    #[test]
    fn tool_node_reads_the_handler_from_its_own_bucket() {
        let node = tool_node_with_props(json!({ "tool": { "handler": "search_nodes" } }));
        assert!(validate_native_tool(&node).is_err());
    }

    #[test]
    fn tool_node_rejects_empty_content() {
        let mut node = tool_node_with_props(json!({
            "tool-native": { "handler": "search_nodes" }
        }));
        node.content = "".to_string();
        assert!(ToolNodeBehavior.validate(&node).is_err());
        assert!(validate_native_tool(&node).is_err());
    }

    /// The trust gate is the subtype's rule (ADR-086 §12): a native tool is
    /// always offered, and any other tool subtype only when `enabled`.
    #[test]
    fn the_trust_gate_is_decided_by_the_tool_subtype() {
        for enabled in [true, false] {
            assert!(tool_is_offered(&NATIVE_TOOL_CHAIN, enabled));
            // A subtype of the native tool is native too.
            assert!(tool_is_offered(
                &["tool-native-plus", "tool-native", "tool"],
                enabled
            ));
            assert_eq!(tool_is_offered(&["tool-remote", "tool"], enabled), enabled);
            // The bare base is no more trusted than any other tool that is
            // not native, and a type that is no tool is never offered.
            assert_eq!(tool_is_offered(&["tool"], enabled), enabled);
            assert!(!tool_is_offered(&["text"], enabled));
            assert!(!tool_is_offered(&["invoice"], enabled));
        }
        assert_eq!(ToolOrigin::of(&NATIVE_TOOL_CHAIN), Some(ToolOrigin::Native));
        assert_eq!(
            ToolOrigin::of(&["tool-remote", "tool"]),
            Some(ToolOrigin::External)
        );
        assert_eq!(ToolOrigin::of(&["invoice"]), None);
    }

    /// The registry resolves a native tool's embedding and markdown rules
    /// from the nearest behaviour in its chain, and they are the base's.
    #[test]
    fn a_native_tool_takes_the_bases_embedding_rules() {
        let registry = NodeBehaviorRegistry::new();
        let node = tool_node_with_props(json!({
            "tool": { "description": "Search nodes by keyword" },
            "tool-native": { "handler": "search_nodes" },
        }));
        let resolved = registry.resolve(&NATIVE_TOOL_CHAIN);
        assert_eq!(resolved.type_name(), "tool-native");
        assert_eq!(
            resolved.get_embeddable_content(&node),
            ToolNodeBehavior.get_embeddable_content(&node)
        );
        assert!(resolved
            .get_embeddable_content(&node)
            .unwrap()
            .contains("Search nodes by keyword"));
        assert!(!resolved.supports_markdown());
        assert!(resolved.get_parent_contribution(&node).is_none());
        let chain: Vec<&str> = registry
            .for_chain(&NATIVE_TOOL_CHAIN)
            .iter()
            .map(|b| b.type_name())
            .collect();
        assert_eq!(chain, ["tool", "tool-native"]);
    }

    #[test]
    fn tool_node_rejects_schema_depth_exceeded() {
        let behavior = ToolNodeBehavior;
        // Nest 7 levels deep — exceeds MAX_SCHEMA_DEPTH (5)
        let deep = json!({
            "type": "object",
            "properties": {
                "a": {
                    "type": "object",
                    "properties": {
                        "b": {
                            "type": "object",
                            "properties": {
                                "c": {
                                    "type": "object",
                                    "properties": {
                                        "d": {
                                            "type": "object",
                                            "properties": {
                                                "e": { "type": "string" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        let node = tool_node_with_props(json!({
            "tool": { "parameter_schema": deep },
            "tool-native": { "handler": "deep_tool" },
        }));
        assert!(
            behavior.validate(&node).is_err(),
            "Schema exceeding depth limit should be rejected"
        );
    }

    #[test]
    fn tool_node_rejects_unbounded_additional_properties() {
        let behavior = ToolNodeBehavior;
        // The guard is the base's, so a subtype that is not native is held to
        // it too.
        let node = Node::new(
            "tool-remote".to_string(),
            "bad_tool".to_string(),
            json!({
                "tool": {
                    "parameter_schema": {
                        "type": "object",
                        "additionalProperties": true,
                    },
                }
            }),
        );
        assert!(
            behavior.validate(&node).is_err(),
            "Schema with additionalProperties:true should be rejected"
        );
        assert!(NodeBehaviorRegistry::new()
            .validate_node(&node, &["tool-remote", "tool"])
            .is_err());
    }

    /// The depth limit must admit the deepest LEGITIMATE tool schema. The real
    /// `create_schema` enum-field path
    /// `properties → fields → items → properties → coreValues → items →
    /// properties → label` reaches object-nesting depth 8 (the recursion counts
    /// every object-valued key). `MAX_SCHEMA_DEPTH` was 5, which rejected it and
    /// aborted seeding of create_schema + all tools after it. No subtype is
    /// exempt from the guard (ADR-036: one trust model) — the fix is a limit
    /// that fits real tools. Regression for the seed missing-tools bug.
    #[test]
    fn tool_node_accepts_create_schema_real_depth() {
        let behavior = ToolNodeBehavior;
        // Mirrors create_schema's deepest valid path: an enum field whose
        // coreValues items have a `label` — bounded (no additionalProperties),
        // depth 8.
        let create_schema_shaped = json!({
            "type": "object",
            "properties": {
                "fields": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "coreValues": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        let node = tool_node_with_props(json!({
            "tool": { "parameter_schema": create_schema_shaped },
            "tool-native": { "handler": "create_schema" },
        }));
        assert!(
            behavior.validate(&node).is_ok(),
            "A real-shaped create_schema parameter_schema (depth 8) must validate under the limit"
        );
    }

    /// `additionalProperties: true` is rejected for a native tool too: being
    /// trusted to be offered is not an exemption from the schema guard
    /// (ADR-036).
    #[test]
    fn tool_node_rejects_unbounded_additional_properties_even_when_native() {
        let node = tool_node_with_props(json!({
            "tool": {
                "parameter_schema": {
                    "type": "object",
                    "additionalProperties": true,
                },
            },
            "tool-native": { "handler": "sneaky_tool" },
        }));
        assert!(
            validate_native_tool(&node).is_err(),
            "additionalProperties:true must be rejected for a native tool as well"
        );
    }

    #[test]
    fn tool_node_embeddable_content_uses_name_and_description() {
        let behavior = ToolNodeBehavior;
        let node = tool_node_with_props(json!({
            "tool": { "description": "Search nodes by keyword" },
            "tool-native": { "handler": "search_nodes" },
        }));
        let content = behavior.get_embeddable_content(&node).unwrap();
        assert!(content.contains("search_nodes"));
        assert!(content.contains("Search nodes by keyword"));
    }

    #[test]
    fn tool_node_embeddable_content_none_when_empty() {
        let behavior = ToolNodeBehavior;
        let mut node = tool_node_with_props(json!({}));
        node.content = "".to_string();
        assert!(behavior.get_embeddable_content(&node).is_none());
    }

    #[test]
    fn tool_node_parent_contribution_is_none() {
        let behavior = ToolNodeBehavior;
        let node = tool_node_with_props(json!({ "tool-native": { "handler": "search_nodes" } }));
        assert!(behavior.get_parent_contribution(&node).is_none());
    }

    // --- PersonNodeBehavior tests ---

    fn person_node(props: serde_json::Value) -> Node {
        Node::new("person".to_string(), String::new(), props)
    }

    #[test]
    fn person_schema_is_present_in_core_schemas() {
        use crate::models::core_schemas::get_core_schemas;
        let schemas = get_core_schemas();
        assert!(schemas.iter().any(|s| s.envelope.id == "person"));
    }

    #[test]
    fn person_behavior_is_registered() {
        let registry = NodeBehaviorRegistry::new();
        let types = registry.get_all_types();
        assert!(types.contains(&"person".to_string()));
    }

    #[test]
    fn person_empty_name_is_valid() {
        let behavior = PersonNodeBehavior;
        let node = person_node(json!({"person": {}}));
        assert!(behavior.validate(&node).is_ok());
    }

    #[test]
    fn person_absent_name_is_valid() {
        let behavior = PersonNodeBehavior;
        let node = person_node(json!({}));
        assert!(behavior.validate(&node).is_ok());
    }

    #[test]
    fn person_valid_email_passes() {
        let behavior = PersonNodeBehavior;
        let node = person_node(json!({"person": {"email": "alice@example.com"}}));
        assert!(behavior.validate(&node).is_ok());
    }

    #[test]
    fn person_invalid_email_fails() {
        let behavior = PersonNodeBehavior;
        let node = person_node(json!({"person": {"email": "notanemail"}}));
        assert!(behavior.validate(&node).is_err());
    }

    #[test]
    fn person_email_missing_dot_fails() {
        let behavior = PersonNodeBehavior;
        let node = person_node(json!({"person": {"email": "alice@example"}}));
        assert!(behavior.validate(&node).is_err());
    }

    #[test]
    fn person_empty_email_is_valid() {
        let behavior = PersonNodeBehavior;
        let node = person_node(json!({"person": {"email": ""}}));
        assert!(behavior.validate(&node).is_ok());
    }

    #[test]
    fn person_is_not_embedded() {
        let behavior = PersonNodeBehavior;
        let node = person_node(json!({"person": {"first_name": "Bob"}}));
        assert_eq!(behavior.get_embeddable_content(&node), None);
        assert_eq!(behavior.get_parent_contribution(&node), None);
    }

    // --- DatabaseSettingsNodeBehavior tests ---

    fn database_settings_node(props: serde_json::Value) -> Node {
        Node::new("database-settings".to_string(), String::new(), props)
    }

    #[test]
    fn database_settings_schema_is_present_in_core_schemas() {
        use crate::models::core_schemas::get_core_schemas;
        let schemas = get_core_schemas();
        assert!(schemas.iter().any(|s| s.envelope.id == "database-settings"));
    }

    #[test]
    fn database_settings_behavior_is_registered() {
        let registry = NodeBehaviorRegistry::new();
        let types = registry.get_all_types();
        assert!(types.contains(&"database-settings".to_string()));
        assert_eq!(
            registry.get("database-settings").unwrap().type_name(),
            "database-settings"
        );
    }

    #[test]
    fn database_settings_accepts_a_list_of_strings_as_required_extensions() {
        let behavior = DatabaseSettingsNodeBehavior;
        for value in [
            json!([]),
            json!(["fixture"]),
            json!(["a", "b"]),
            json!(null),
        ] {
            for props in [
                json!({ "database-settings": { "required_extensions": value.clone() } }),
                json!({ "required_extensions": value.clone() }),
            ] {
                let node = database_settings_node(props.clone());
                assert!(behavior.validate(&node).is_ok(), "{props} must be valid");
            }
        }
    }

    #[test]
    fn database_settings_rejects_required_extensions_that_are_not_strings() {
        let behavior = DatabaseSettingsNodeBehavior;
        for value in [
            json!(["fixture", 1]),
            json!("fixture"),
            json!(42),
            json!({"a": "b"}),
        ] {
            for props in [
                json!({ "database-settings": { "required_extensions": value.clone() } }),
                json!({ "required_extensions": value.clone() }),
            ] {
                let node = database_settings_node(props.clone());
                assert!(
                    behavior.validate(&node).is_err(),
                    "{props} must be rejected"
                );
            }
        }
    }

    #[test]
    fn database_settings_minimal_node_is_valid() {
        let behavior = DatabaseSettingsNodeBehavior;
        let node = database_settings_node(json!({}));
        assert!(behavior.validate(&node).is_ok());
    }

    #[test]
    fn database_settings_capabilities() {
        let behavior = DatabaseSettingsNodeBehavior;
        assert_eq!(behavior.type_name(), "database-settings");
        assert!(!behavior.supports_markdown());
    }

    #[test]
    fn database_settings_is_not_embeddable() {
        let behavior = DatabaseSettingsNodeBehavior;
        let node = database_settings_node(json!({}));
        assert!(behavior.get_embeddable_content(&node).is_none());
        assert!(behavior.get_parent_contribution(&node).is_none());
    }

    // --- aggregate_children_content: traversal order regression ---

    /// Minimal `NodeAccessor` test double. `with_children` registers a
    /// parent's children in the exact sibling order `get_children` should
    /// return them (already sorted by fractional order at the real
    /// accessor), so this mock never has to reorder anything itself.
    struct MockNodeAccessor {
        children: HashMap<String, Vec<Node>>,
    }

    impl MockNodeAccessor {
        fn new() -> Self {
            Self {
                children: HashMap::new(),
            }
        }

        fn with_children(mut self, parent_id: &str, children: Vec<Node>) -> Self {
            self.children.insert(parent_id.to_string(), children);
            self
        }
    }

    #[async_trait::async_trait]
    impl NodeAccessor for MockNodeAccessor {
        async fn get_node(
            &self,
            _id: &str,
        ) -> Result<Option<Node>, crate::services::error::NodeServiceError> {
            Ok(None)
        }

        async fn get_children(
            &self,
            parent_id: &str,
        ) -> Result<Vec<Node>, crate::services::error::NodeServiceError> {
            Ok(self.children.get(parent_id).cloned().unwrap_or_default())
        }

        async fn get_nodes(
            &self,
            _ids: &[&str],
        ) -> Result<Vec<Node>, crate::services::error::NodeServiceError> {
            Ok(Vec::new())
        }

        async fn access_boundaries_under(
            &self,
            _root_id: &str,
        ) -> Result<HashSet<String>, crate::services::error::NodeServiceError> {
            Ok(HashSet::new())
        }

        async fn type_chain(
            &self,
            node_type: &str,
        ) -> Result<Vec<String>, crate::services::error::NodeServiceError> {
            Ok(vec![node_type.to_string()])
        }
    }

    /// Regression test: aggregation must read in natural top-to-bottom
    /// document order (pre-order depth-first), not breadth-first / level
    /// order. For `root -> [A -> [A1, A2], B -> [B1]]`, the correct order is
    /// exactly `A, A1, A2, B, B1` — a child's whole subtree before its next
    /// sibling. The bug this guards against produced `A, B, B1, A1, A2`
    /// (later siblings' subtrees popped off the stack before earlier
    /// siblings' own children were reached).
    #[tokio::test]
    async fn test_aggregate_children_content_preorder_document_order() {
        let root = Node::new_with_id(
            "root".to_string(),
            "text".to_string(),
            "Root".to_string(),
            json!({}),
        );
        let a = Node::new_with_id(
            "a".to_string(),
            "text".to_string(),
            "A".to_string(),
            json!({}),
        );
        let a1 = Node::new_with_id(
            "a1".to_string(),
            "text".to_string(),
            "A1".to_string(),
            json!({}),
        );
        let a2 = Node::new_with_id(
            "a2".to_string(),
            "text".to_string(),
            "A2".to_string(),
            json!({}),
        );
        let b = Node::new_with_id(
            "b".to_string(),
            "text".to_string(),
            "B".to_string(),
            json!({}),
        );
        let b1 = Node::new_with_id(
            "b1".to_string(),
            "text".to_string(),
            "B1".to_string(),
            json!({}),
        );

        let accessor = MockNodeAccessor::new()
            .with_children("root", vec![a, b])
            .with_children("a", vec![a1, a2])
            .with_children("b", vec![b1]);

        let registry = NodeBehaviorRegistry::new();

        let result = aggregate_children_content(&root, &accessor, &registry)
            .await
            .expect("expected aggregated content for a tree with content-bearing children");

        assert_eq!(
            result, "A\n\nA1\n\nA2\n\nB\n\nB1",
            "aggregated content must read in document order (pre-order DFS), not breadth-first"
        );
    }

    /// Aggregation stops at a chat: a chat's subtree is not embedded, and each
    /// child of one is its own embedding root, so it must not also be folded
    /// into the vector of the page the chat sits in. A subtype of a chat is
    /// held to the same rule.
    #[tokio::test]
    async fn test_aggregate_children_content_stops_at_a_chat() {
        struct ChainAccessor(MockNodeAccessor);

        #[async_trait::async_trait]
        impl crate::services::NodeAccessor for ChainAccessor {
            async fn get_node(
                &self,
                id: &str,
            ) -> Result<Option<Node>, crate::services::error::NodeServiceError> {
                self.0.get_node(id).await
            }

            async fn get_children(
                &self,
                parent_id: &str,
            ) -> Result<Vec<Node>, crate::services::error::NodeServiceError> {
                self.0.get_children(parent_id).await
            }

            async fn get_nodes(
                &self,
                ids: &[&str],
            ) -> Result<Vec<Node>, crate::services::error::NodeServiceError> {
                self.0.get_nodes(ids).await
            }

            async fn access_boundaries_under(
                &self,
                root_id: &str,
            ) -> Result<HashSet<String>, crate::services::error::NodeServiceError> {
                self.0.access_boundaries_under(root_id).await
            }

            async fn type_chain(
                &self,
                node_type: &str,
            ) -> Result<Vec<String>, crate::services::error::NodeServiceError> {
                Ok(match node_type {
                    "support-chat" => vec!["support-chat".to_string(), "ai-chat".to_string()],
                    other => vec![other.to_string()],
                })
            }
        }

        let node = |id: &str, node_type: &str, content: &str| {
            Node::new_with_id(
                id.to_string(),
                node_type.to_string(),
                content.to_string(),
                json!({}),
            )
        };
        let root = node("root", "text", "Root");
        let accessor = ChainAccessor(
            MockNodeAccessor::new()
                .with_children(
                    "root",
                    vec![
                        node("before", "text", "Before"),
                        node("chat", "ai-chat", "A chat"),
                        node("support", "support-chat", "A support chat"),
                        node("after", "text", "After"),
                    ],
                )
                .with_children("chat", vec![node("kept", "text", "Kept in the chat")])
                .with_children(
                    "support",
                    vec![node("kept2", "text", "Kept in the support chat")],
                )
                .with_children("after", vec![node("below", "text", "Below")]),
        );

        let result = aggregate_children_content(&root, &accessor, &NodeBehaviorRegistry::new())
            .await
            .expect("the page's own lines still aggregate");

        assert!(
            result.contains("Before") && result.contains("Below"),
            "{result}"
        );
        assert!(
            !result.contains("Kept in the"),
            "a chat's children are their own embedding roots: {result}"
        );
    }
}
