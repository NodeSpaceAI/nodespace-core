//! Collection Service
//!
//! This module provides utilities and services for working with collections,
//! including path parsing, membership management, and DAG validation.
//!
//! ## Path Syntax
//!
//! Collections use colon (`:`) as a path delimiter:
//! - Valid: `hr:policy:vacation:Berlin`
//! - Valid: `engineering:docs`
//! - Valid: `Berlin` (single segment)
//! - Invalid: `hr::policy` (empty segment)
//! - Invalid: `:hr:policy` (leading colon)
//! - Invalid: `hr:policy:` (trailing colon)
//!
//! ## Architecture
//!
//! - Collections form a DAG (Directed Acyclic Graph)
//! - Collection names are globally unique
//! - Case-insensitive lookup, preserves original case on create
//! - Maximum depth of 10 levels

use super::error::NodeServiceError;

/// Maximum allowed depth for collection paths
pub const MAX_COLLECTION_DEPTH: usize = 10;

/// Path delimiter for collection paths
pub const COLLECTION_PATH_DELIMITER: char = ':';

/// A parsed collection path segment
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionSegment {
    /// The original text of the segment (preserves case)
    pub name: String,
    /// Normalized name for lookup (lowercase)
    pub normalized_name: String,
}

impl CollectionSegment {
    /// Create a new segment from a name
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        let normalized_name = name.to_lowercase();
        Self {
            name,
            normalized_name,
        }
    }
}

/// A parsed collection path
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionPath {
    /// The path segments from root to leaf
    pub segments: Vec<CollectionSegment>,
    /// The original path string
    pub original: String,
}

impl CollectionPath {
    /// Get the depth of this path (number of segments)
    pub fn depth(&self) -> usize {
        self.segments.len()
    }

    /// Get the final segment (collection name)
    pub fn final_segment(&self) -> Option<&CollectionSegment> {
        self.segments.last()
    }

    /// Get the parent path (all segments except the last)
    pub fn parent(&self) -> Option<CollectionPath> {
        if self.segments.len() <= 1 {
            return None;
        }

        let parent_segments: Vec<CollectionSegment> =
            self.segments[..self.segments.len() - 1].to_vec();
        let parent_original = parent_segments
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>()
            .join(&COLLECTION_PATH_DELIMITER.to_string());

        Some(CollectionPath {
            segments: parent_segments,
            original: parent_original,
        })
    }

    /// Check if this path is an ancestor of another path
    pub fn is_ancestor_of(&self, other: &CollectionPath) -> bool {
        if self.segments.len() >= other.segments.len() {
            return false;
        }

        self.segments
            .iter()
            .zip(other.segments.iter())
            .all(|(a, b)| a.normalized_name == b.normalized_name)
    }

    /// Check if this path is a descendant of another path
    pub fn is_descendant_of(&self, other: &CollectionPath) -> bool {
        other.is_ancestor_of(self)
    }
}

/// Parse a collection path string into segments
///
/// # Arguments
///
/// * `path` - The path string to parse (e.g., "hr:policy:vacation")
///
/// # Returns
///
/// * `Ok(CollectionPath)` - Successfully parsed path
/// * `Err(NodeServiceError)` - Invalid path format
///
/// # Validation Rules
///
/// 1. Path cannot be empty
/// 2. No empty segments (consecutive colons `::`)
/// 3. No leading colon (`:hr:policy`)
/// 4. No trailing colon (`hr:policy:`)
/// 5. No segment can contain a colon
/// 6. Maximum depth of 10 levels
/// 7. Each segment is trimmed of whitespace
///
/// # Examples
///
/// ```
/// use nodespace_core::services::collection_service::parse_collection_path;
///
/// // Valid paths
/// let path = parse_collection_path("hr:policy:vacation").unwrap();
/// assert_eq!(path.depth(), 3);
///
/// let single = parse_collection_path("engineering").unwrap();
/// assert_eq!(single.depth(), 1);
///
/// // Invalid paths
/// assert!(parse_collection_path("").is_err());
/// assert!(parse_collection_path("hr::policy").is_err());
/// assert!(parse_collection_path(":hr:policy").is_err());
/// assert!(parse_collection_path("hr:policy:").is_err());
/// ```
pub fn parse_collection_path(path: &str) -> Result<CollectionPath, NodeServiceError> {
    let trimmed = path.trim();

    // Rule 1: Path cannot be empty
    if trimmed.is_empty() {
        return Err(NodeServiceError::invalid_collection_path(
            "path cannot be empty",
        ));
    }

    // Rule 3: No leading colon
    if trimmed.starts_with(COLLECTION_PATH_DELIMITER) {
        return Err(NodeServiceError::invalid_collection_path(
            "path cannot start with ':'",
        ));
    }

    // Rule 4: No trailing colon
    if trimmed.ends_with(COLLECTION_PATH_DELIMITER) {
        return Err(NodeServiceError::invalid_collection_path(
            "path cannot end with ':'",
        ));
    }

    // Split and process segments
    let raw_segments: Vec<&str> = trimmed.split(COLLECTION_PATH_DELIMITER).collect();

    // Rule 6: Maximum depth check
    if raw_segments.len() > MAX_COLLECTION_DEPTH {
        return Err(NodeServiceError::collection_depth_exceeded(
            trimmed,
            MAX_COLLECTION_DEPTH,
        ));
    }

    let mut segments = Vec::with_capacity(raw_segments.len());

    for (i, raw) in raw_segments.iter().enumerate() {
        let segment_name = raw.trim();

        // Rule 2: No empty segments (also catches Rule 3 & 4 edge cases)
        if segment_name.is_empty() {
            return Err(NodeServiceError::invalid_collection_path(format!(
                "empty segment at position {} in path '{}'",
                i + 1,
                trimmed
            )));
        }

        // Rule 5: Segment cannot contain colon (already enforced by split, but check anyway)
        if segment_name.contains(COLLECTION_PATH_DELIMITER) {
            return Err(NodeServiceError::invalid_collection_path(format!(
                "segment '{}' cannot contain ':'",
                segment_name
            )));
        }

        segments.push(CollectionSegment::new(segment_name));
    }

    Ok(CollectionPath {
        segments,
        original: trimmed.to_string(),
    })
}

/// Validate a single collection name (not a path)
///
/// # Validation Rules
///
/// 1. Name cannot be empty
/// 2. Name cannot contain the path delimiter (`:`)
/// 3. Name is trimmed of whitespace
///
/// # Examples
///
/// ```
/// use nodespace_core::services::collection_service::validate_collection_name;
///
/// assert!(validate_collection_name("engineering").is_ok());
/// assert!(validate_collection_name("Human Resources (HR)").is_ok());
/// assert!(validate_collection_name("hr:policy").is_err()); // Contains colon
/// assert!(validate_collection_name("").is_err()); // Empty
/// ```
pub fn validate_collection_name(name: &str) -> Result<String, NodeServiceError> {
    let trimmed = name.trim();

    if trimmed.is_empty() {
        return Err(NodeServiceError::invalid_collection_path(
            "collection name cannot be empty",
        ));
    }

    if trimmed.contains(COLLECTION_PATH_DELIMITER) {
        return Err(NodeServiceError::invalid_collection_path(format!(
            "collection name '{}' cannot contain ':' (use it in paths only)",
            trimmed
        )));
    }

    Ok(trimmed.to_string())
}

/// Normalize a collection name for case-insensitive lookup
pub fn normalize_collection_name(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Build a path string from segments
pub fn build_path_string(segments: &[&str]) -> String {
    segments.join(&COLLECTION_PATH_DELIMITER.to_string())
}

// ============================================================================
// CollectionService - High-level operations that integrate with the store
// ============================================================================

use crate::db::events::{CollectionMembership, DomainEvent};
use crate::db::{DatabaseError, SurrealStore};
use crate::models::Node;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Helper to convert anyhow errors to NodeServiceError with database context
fn db_error(e: anyhow::Error, context: &str) -> NodeServiceError {
    NodeServiceError::DatabaseError(DatabaseError::SqlExecutionError {
        context: format!("{}: {}", context, e),
    })
}

/// Result of resolving a collection path
#[derive(Debug, Clone)]
pub struct ResolvedCollection {
    /// The collection node
    pub node: Node,
    /// Whether the collection was created (true) or already existed (false)
    pub created: bool,
}

/// Result of resolving a full path
#[derive(Debug, Clone)]
pub struct ResolvedPath {
    /// All collections in the path, from root to leaf
    pub collections: Vec<ResolvedCollection>,
    /// The final (leaf) collection
    pub leaf: Node,
}

impl ResolvedPath {
    /// Get the ID of the leaf collection
    pub fn leaf_id(&self) -> &str {
        &self.leaf.id
    }
}

/// High-level collection operations
///
/// This service provides path resolution, membership management, and collection
/// queries. It uses `SurrealStore` for database operations.
///
/// # Event Emission
///
/// CollectionService emits domain events when collection membership changes:
/// - `CollectionMemberAdded` - when a node is added to a collection
/// - `CollectionMemberRemoved` - when a node is removed from a collection
///
/// Events include `source_client_id` for filtering (prevents feedback loops).
pub struct CollectionService<'a, C = surrealdb::engine::local::Db>
where
    C: surrealdb::Connection,
{
    store: &'a Arc<SurrealStore<C>>,
    /// Optional event sender for broadcasting domain events
    event_tx: Option<broadcast::Sender<DomainEvent>>,
    /// Optional client identifier for event source tracking
    client_id: Option<String>,
}

impl<'a, C> CollectionService<'a, C>
where
    C: surrealdb::Connection,
{
    /// Create a new CollectionService without event emission
    ///
    /// Use `with_events()` if you need to emit domain events for membership changes.
    pub fn new(store: &'a Arc<SurrealStore<C>>) -> Self {
        Self {
            store,
            event_tx: None,
            client_id: None,
        }
    }

    /// Create a CollectionService with event emission support
    ///
    /// # Arguments
    ///
    /// * `store` - The database store
    /// * `event_tx` - Broadcast sender for domain events
    /// * `client_id` - Optional client ID for event source tracking
    pub fn with_events(
        store: &'a Arc<SurrealStore<C>>,
        event_tx: broadcast::Sender<DomainEvent>,
        client_id: Option<String>,
    ) -> Self {
        Self {
            store,
            event_tx: Some(event_tx),
            client_id,
        }
    }

    /// Emit a domain event to all subscribers
    ///
    /// Internal helper for emitting events after successful operations.
    /// No-op if event_tx is not configured.
    fn emit_event(&self, event: DomainEvent) {
        if let Some(tx) = &self.event_tx {
            let _ = tx.send(event);
        }
    }

    /// Resolve a collection path, creating collections as needed
    ///
    /// This method parses the path, finds or creates each segment in order,
    /// and establishes parent-child relationships between them.
    ///
    /// # Arguments
    ///
    /// * `path` - The collection path (e.g., "hr:policy:vacation")
    ///
    /// # Returns
    ///
    /// The resolved path with all collections, indicating which were created
    ///
    /// # Example
    ///
    /// ```ignore
    /// let service = CollectionService::new(&store);
    /// let resolved = service.resolve_path("hr:policy:vacation").await?;
    /// println!("Leaf collection: {}", resolved.leaf.content);
    /// ```
    pub async fn resolve_path(&self, path: &str) -> Result<ResolvedPath, NodeServiceError> {
        let parsed = parse_collection_path(path)?;

        // Batch fetch all existing collections in one query
        let segment_names: Vec<String> = parsed.segments.iter().map(|s| s.name.clone()).collect();
        let existing_collections = self
            .store
            .get_collections_by_names(&segment_names)
            .await
            .map_err(|e| db_error(e, "Failed to batch fetch collections"))?;

        let mut collections = Vec::with_capacity(parsed.segments.len());

        // Collections are flat - paths are just naming conventions for navigation.
        // Each segment is an independent collection with a globally unique name.
        // No parent-child hierarchy between collections - a collection can be
        // referenced via multiple paths (e.g., "hr:policy:vacation:berlin" and
        // "engineering:office:berlin" both reference the same "berlin" collection).
        for segment in &parsed.segments {
            // Check if this segment exists (case-insensitive via normalized name)
            let (node, created) = match existing_collections.get(&segment.normalized_name) {
                Some(existing) => (existing.clone(), false),
                None => {
                    // Create new collection (no parent - collections are flat)
                    let new_node = self.create_collection(&segment.name, None).await?;
                    (new_node, true)
                }
            };

            collections.push(ResolvedCollection { node, created });
        }

        let leaf = collections.last().unwrap().node.clone();
        Ok(ResolvedPath { collections, leaf })
    }

    /// Find a collection by path without creating it
    ///
    /// Returns the leaf collection if the entire path exists, None otherwise.
    pub async fn find_collection_by_path(
        &self,
        path: &str,
    ) -> Result<Option<Node>, NodeServiceError> {
        let parsed = parse_collection_path(path)?;

        // For a path to exist, all segments must exist as collections
        // We just need to find the final segment since names are globally unique
        let final_segment = parsed
            .final_segment()
            .ok_or_else(|| NodeServiceError::invalid_collection_path("path has no segments"))?;

        self.store
            .get_collection_by_name(&final_segment.name)
            .await
            .map_err(|e| db_error(e, "Failed to find collection by path"))
    }

    /// Create a collection node
    ///
    /// Collections are flat (no hierarchy between them). Paths like "hr:policy:vacation"
    /// are naming conventions for navigation, not structural relationships.
    ///
    /// # Arguments
    ///
    /// * `name` - The collection name (will be validated, must be globally unique)
    async fn create_collection(
        &self,
        name: &str,
        _parent_id: Option<&str>,
    ) -> Result<Node, NodeServiceError> {
        let validated_name = validate_collection_name(name)?;

        // Create the collection node (no parent - collections are flat)
        let node = Node::new("collection".to_string(), validated_name, json!({}));

        // Create in database
        let created = self
            .store
            .create_node(node, None)
            .await
            .map_err(|e| db_error(e, "Failed to create collection node"))?;

        Ok(created)
    }

    /// Add a node to a collection by path
    ///
    /// Resolves the path (creating collections as needed) and adds the node
    /// as a member of the leaf collection.
    ///
    /// # Arguments
    ///
    /// * `node_id` - The ID of the node to add
    /// * `collection_path` - The collection path
    pub async fn add_to_collection_by_path(
        &self,
        node_id: &str,
        collection_path: &str,
    ) -> Result<ResolvedPath, NodeServiceError> {
        let resolved = self.resolve_path(collection_path).await?;

        self.store
            .add_to_collection(node_id, &resolved.leaf.id)
            .await
            .map_err(|e| db_error(e, "Failed to add node to collection"))?;

        // Emit CollectionMemberAdded event
        self.emit_event(DomainEvent::CollectionMemberAdded {
            membership: CollectionMembership {
                member_id: node_id.to_string(),
                collection_id: resolved.leaf.id.clone(),
            },
            source_client_id: self.client_id.clone(),
        });

        Ok(resolved)
    }

    /// Add a node to a collection by ID
    ///
    /// # Arguments
    ///
    /// * `node_id` - The ID of the node to add
    /// * `collection_id` - The ID of the collection
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The collection node doesn't exist
    /// - The target node is not a collection type
    pub async fn add_to_collection(
        &self,
        node_id: &str,
        collection_id: &str,
    ) -> Result<(), NodeServiceError> {
        // Validate that the target is actually a collection node
        let collection_node = self
            .store
            .get_node(collection_id)
            .await
            .map_err(|e| db_error(e, "Failed to fetch collection node"))?
            .ok_or_else(|| {
                NodeServiceError::CollectionNotFound(format!(
                    "Collection not found: '{}'",
                    collection_id
                ))
            })?;

        if collection_node.node_type != "collection" {
            return Err(NodeServiceError::InvalidCollectionPath(format!(
                "Cannot add member to non-collection node: '{}' has type '{}'",
                collection_id, collection_node.node_type
            )));
        }

        self.store
            .add_to_collection(node_id, collection_id)
            .await
            .map_err(|e| db_error(e, "Failed to add node to collection"))?;

        // Emit CollectionMemberAdded event
        self.emit_event(DomainEvent::CollectionMemberAdded {
            membership: CollectionMembership {
                member_id: node_id.to_string(),
                collection_id: collection_id.to_string(),
            },
            source_client_id: self.client_id.clone(),
        });

        Ok(())
    }

    /// Remove a node from a collection
    ///
    /// # Arguments
    ///
    /// * `node_id` - The ID of the node to remove
    /// * `collection_id` - The ID of the collection
    pub async fn remove_from_collection(
        &self,
        node_id: &str,
        collection_id: &str,
    ) -> Result<(), NodeServiceError> {
        self.store
            .remove_from_collection(node_id, collection_id)
            .await
            .map_err(|e| db_error(e, "Failed to remove node from collection"))?;

        // Emit CollectionMemberRemoved event
        self.emit_event(DomainEvent::CollectionMemberRemoved {
            membership: CollectionMembership {
                member_id: node_id.to_string(),
                collection_id: collection_id.to_string(),
            },
            source_client_id: self.client_id.clone(),
        });

        Ok(())
    }

    /// Get all collections a node belongs to
    ///
    /// Returns the IDs of all collections the node is a member of.
    pub async fn get_node_collections(
        &self,
        node_id: &str,
    ) -> Result<Vec<String>, NodeServiceError> {
        self.store
            .get_node_memberships(node_id)
            .await
            .map_err(|e| db_error(e, "Failed to get node collections"))
    }

    /// Get all members of a collection
    ///
    /// Returns the IDs of all nodes that are members of the collection.
    pub async fn get_collection_members(
        &self,
        collection_id: &str,
    ) -> Result<Vec<String>, NodeServiceError> {
        self.store
            .get_collection_members(collection_id)
            .await
            .map_err(|e| db_error(e, "Failed to get collection members"))
    }

    /// Get all members of a collection recursively
    ///
    /// Returns the IDs of all nodes that are members of the collection
    /// or any of its descendant collections.
    pub async fn get_collection_members_recursive(
        &self,
        collection_id: &str,
    ) -> Result<Vec<String>, NodeServiceError> {
        self.store
            .get_collection_members_recursive(collection_id)
            .await
            .map_err(|e| db_error(e, "Failed to get recursive collection members"))
    }

    /// Get a collection by name (case-insensitive)
    pub async fn get_collection_by_name(
        &self,
        name: &str,
    ) -> Result<Option<Node>, NodeServiceError> {
        self.store
            .get_collection_by_name(name)
            .await
            .map_err(|e| db_error(e, "Failed to get collection by name"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // parse_collection_path tests
    // ========================================================================

    #[test]
    fn test_parse_simple_path() {
        let path = parse_collection_path("engineering").unwrap();
        assert_eq!(path.depth(), 1);
        assert_eq!(path.segments[0].name, "engineering");
        assert_eq!(path.segments[0].normalized_name, "engineering");
    }

    #[test]
    fn test_parse_multi_segment_path() {
        let path = parse_collection_path("hr:policy:vacation").unwrap();
        assert_eq!(path.depth(), 3);
        assert_eq!(path.segments[0].name, "hr");
        assert_eq!(path.segments[1].name, "policy");
        assert_eq!(path.segments[2].name, "vacation");
    }

    #[test]
    fn test_parse_preserves_case() {
        let path = parse_collection_path("HR:Policy:Vacation").unwrap();
        assert_eq!(path.segments[0].name, "HR");
        assert_eq!(path.segments[0].normalized_name, "hr");
        assert_eq!(path.segments[1].name, "Policy");
        assert_eq!(path.segments[1].normalized_name, "policy");
    }

    #[test]
    fn test_parse_trims_whitespace() {
        let path = parse_collection_path("  hr : policy : vacation  ").unwrap();
        assert_eq!(path.segments[0].name, "hr");
        assert_eq!(path.segments[1].name, "policy");
        assert_eq!(path.segments[2].name, "vacation");
    }

    #[test]
    fn test_parse_with_spaces_in_name() {
        let path = parse_collection_path("Human Resources (HR):Policy:Vacation").unwrap();
        assert_eq!(path.segments[0].name, "Human Resources (HR)");
        assert_eq!(path.segments[0].normalized_name, "human resources (hr)");
    }

    #[test]
    fn test_parse_rejects_empty_path() {
        let result = parse_collection_path("");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("path cannot be empty"));
    }

    #[test]
    fn test_parse_rejects_whitespace_only_path() {
        let result = parse_collection_path("   ");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("path cannot be empty"));
    }

    #[test]
    fn test_parse_rejects_leading_colon() {
        let result = parse_collection_path(":hr:policy");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cannot start with ':'"));
    }

    #[test]
    fn test_parse_rejects_trailing_colon() {
        let result = parse_collection_path("hr:policy:");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cannot end with ':'"));
    }

    #[test]
    fn test_parse_rejects_empty_segment() {
        let result = parse_collection_path("hr::policy");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty segment"));
    }

    #[test]
    fn test_parse_rejects_whitespace_segment() {
        let result = parse_collection_path("hr:   :policy");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty segment"));
    }

    #[test]
    fn test_parse_rejects_exceeding_max_depth() {
        // Create a path with 11 segments (exceeds MAX_COLLECTION_DEPTH = 10)
        let segments: Vec<&str> = (0..11)
            .map(|i| if i % 2 == 0 { "a" } else { "b" })
            .collect();
        let deep_path = segments.join(":");
        let result = parse_collection_path(&deep_path);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("exceeds maximum depth"));
    }

    #[test]
    fn test_parse_allows_max_depth() {
        // Create a path with exactly 10 segments (equals MAX_COLLECTION_DEPTH)
        let segments: Vec<&str> = (0..10).map(|_| "level").collect();
        let path = segments.join(":");
        let result = parse_collection_path(&path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().depth(), 10);
    }

    // ========================================================================
    // CollectionPath methods tests
    // ========================================================================

    #[test]
    fn test_path_final_segment() {
        let path = parse_collection_path("hr:policy:vacation").unwrap();
        let final_seg = path.final_segment().unwrap();
        assert_eq!(final_seg.name, "vacation");
    }

    #[test]
    fn test_path_final_segment_single() {
        let path = parse_collection_path("engineering").unwrap();
        let final_seg = path.final_segment().unwrap();
        assert_eq!(final_seg.name, "engineering");
    }

    #[test]
    fn test_path_parent() {
        let path = parse_collection_path("hr:policy:vacation").unwrap();
        let parent = path.parent().unwrap();
        assert_eq!(parent.depth(), 2);
        assert_eq!(parent.original, "hr:policy");
        assert_eq!(parent.segments[0].name, "hr");
        assert_eq!(parent.segments[1].name, "policy");
    }

    #[test]
    fn test_path_parent_of_single_is_none() {
        let path = parse_collection_path("engineering").unwrap();
        assert!(path.parent().is_none());
    }

    #[test]
    fn test_path_is_ancestor_of() {
        let parent = parse_collection_path("hr:policy").unwrap();
        let child = parse_collection_path("hr:policy:vacation").unwrap();
        let grandchild = parse_collection_path("hr:policy:vacation:Berlin").unwrap();

        assert!(parent.is_ancestor_of(&child));
        assert!(parent.is_ancestor_of(&grandchild));
        assert!(child.is_ancestor_of(&grandchild));

        // Not ancestors of self
        assert!(!parent.is_ancestor_of(&parent));
        assert!(!child.is_ancestor_of(&child));

        // Not ancestors of unrelated
        let other = parse_collection_path("engineering:docs").unwrap();
        assert!(!parent.is_ancestor_of(&other));
    }

    #[test]
    fn test_path_is_descendant_of() {
        let parent = parse_collection_path("hr:policy").unwrap();
        let child = parse_collection_path("hr:policy:vacation").unwrap();

        assert!(child.is_descendant_of(&parent));
        assert!(!parent.is_descendant_of(&child));
        assert!(!child.is_descendant_of(&child));
    }

    // ========================================================================
    // validate_collection_name tests
    // ========================================================================

    #[test]
    fn test_validate_name_simple() {
        let name = validate_collection_name("engineering").unwrap();
        assert_eq!(name, "engineering");
    }

    #[test]
    fn test_validate_name_with_spaces() {
        let name = validate_collection_name("Human Resources (HR)").unwrap();
        assert_eq!(name, "Human Resources (HR)");
    }

    #[test]
    fn test_validate_name_trims_whitespace() {
        let name = validate_collection_name("  engineering  ").unwrap();
        assert_eq!(name, "engineering");
    }

    #[test]
    fn test_validate_name_rejects_empty() {
        let result = validate_collection_name("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_name_rejects_colon() {
        let result = validate_collection_name("hr:policy");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cannot contain ':'"));
    }

    // ========================================================================
    // normalize_collection_name tests
    // ========================================================================

    #[test]
    fn test_normalize_name() {
        assert_eq!(normalize_collection_name("Engineering"), "engineering");
        assert_eq!(normalize_collection_name("  HR  "), "hr");
        assert_eq!(
            normalize_collection_name("Human Resources (HR)"),
            "human resources (hr)"
        );
    }

    // ========================================================================
    // build_path_string tests
    // ========================================================================

    #[test]
    fn test_build_path_string() {
        assert_eq!(
            build_path_string(&["hr", "policy", "vacation"]),
            "hr:policy:vacation"
        );
        assert_eq!(build_path_string(&["engineering"]), "engineering");
        assert_eq!(build_path_string(&[]), "");
    }
}
