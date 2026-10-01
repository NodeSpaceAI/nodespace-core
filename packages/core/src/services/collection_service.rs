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

/// Stable namespace for deterministic collection ids (UUIDv5). A fixed, arbitrary
/// UUID — do NOT change it: it anchors collection identity across imports and devices.
const COLLECTION_ID_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x8f5b2c1e_9a4d_4e6f_b3a7_1c2d3e4f5a6bu128);

/// Deterministic collection node id from the (globally-unique) name, normalized so
/// case variants converge. Collection names are globally unique, so deriving the id
/// from the name makes independent imports / devices produce the SAME node id for a
/// given collection instead of each minting a fresh random UUID (which synced up as
/// duplicate collection nodes). It also makes a concurrent double-create of the same
/// name resolve to one node.
pub fn deterministic_collection_id(name: &str) -> String {
    uuid::Uuid::new_v5(
        &COLLECTION_ID_NAMESPACE,
        normalize_collection_name(name).as_bytes(),
    )
    .to_string()
}

/// Build a path string from segments
pub fn build_path_string(segments: &[&str]) -> String {
    segments.join(&COLLECTION_PATH_DELIMITER.to_string())
}

// ============================================================================
// CollectionService - High-level operations that integrate with the store
// ============================================================================

use crate::db::{DatabaseError, SqliteStore};
use crate::models::Node;
use serde_json::json;
use std::sync::Arc;

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
/// queries. It delegates to `NodeService` for relationship operations to ensure
/// proper event emission.
///
/// # Event Emission
///
/// Per service-layer-architecture.md, event emission happens in NodeService.
/// CollectionService delegates `member_of` operations to NodeService, which
/// emits RelationshipCreated/RelationshipDeleted events.
pub struct CollectionService<'a> {
    store: &'a Arc<SqliteStore>,
    node_service: &'a super::NodeService,
}

impl<'a> CollectionService<'a> {
    /// Create a new CollectionService
    ///
    /// # Arguments
    ///
    /// * `store` - The database store
    /// * `node_service` - NodeService for relationship operations and event emission
    pub fn new(store: &'a Arc<SqliteStore>, node_service: &'a super::NodeService) -> Self {
        Self {
            store,
            node_service,
        }
    }

    /// Resolve a collection path, creating collections as needed
    ///
    /// This method parses the path, finds or creates each segment in order,
    /// and establishes hierarchy relationships between them using `member_of` edges.
    ///
    /// # Collection Hierarchy
    ///
    /// Collections form a DAG (Directed Acyclic Graph) where:
    /// - Path `hr:policy:vacation` creates hierarchy using `member_of` edges
    /// - `policy` is a member of `hr`, `vacation` is a member of `policy`
    /// - Each collection can have multiple parents (DAG, not tree)
    /// - Collection names are globally unique (case-insensitive)
    ///
    /// # Why `member_of` instead of `has_child`?
    ///
    /// Collections are nodes, and all nodes inherit `has_child` for document tree structure.
    /// However, collection hierarchy uses `member_of` because:
    /// 1. It's the same pattern as nodes belonging to collections (consistent semantics)
    /// 2. It supports multi-parent DAG naturally (a collection can be in multiple paths)
    /// 3. It distinguishes collection hierarchy from document tree hierarchy
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
    /// let service = CollectionService::new(&store, &node_service);
    /// let resolved = service.resolve_path("hr:policy:vacation").await?;
    /// // Creates: vacation -member_of-> policy -member_of-> hr
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
        let mut previous_collection_id: Option<String> = None;

        // Collections form a hierarchical DAG structure.
        // Each segment in the path is a member of the previous segment (its parent).
        // Collection names are globally unique, so the same collection can
        // appear in multiple paths (e.g., "hr:policy:vacation:berlin" and
        // "engineering:offices:berlin" both reference the same "berlin" collection,
        // which would have two parents: "vacation" and "offices").
        for segment in &parsed.segments {
            // Check if this segment exists (case-insensitive via normalized name)
            let (node, created) = match existing_collections.get(&segment.normalized_name) {
                Some(existing) => (existing.clone(), false),
                None => {
                    // Create new collection
                    let new_node = self.create_collection(&segment.name).await?;
                    (new_node, true)
                }
            };

            // Create hierarchy relationship: current collection is member_of parent collection
            // Direction: child -> member_of -> parent (same as nodes belonging to collections)
            // This is idempotent - if the relationship already exists, nothing happens
            // Delegate to NodeService for event emission
            if let Some(parent_id) = &previous_collection_id {
                self.node_service
                    .create_relationship(&node.id, "member_of", parent_id, json!({}))
                    .await
                    .map_err(|e| {
                        NodeServiceError::query_failed(format!(
                            "Failed to create collection hierarchy: {}",
                            e
                        ))
                    })?;
            }

            previous_collection_id = Some(node.id.clone());
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
    /// Creates a new collection node with the given name. Hierarchy relationships
    /// (member_of edges) are established separately in `resolve_path()`.
    ///
    /// # Arguments
    ///
    /// * `name` - The collection name (will be validated, must be globally unique)
    ///
    /// # Event Emission
    ///
    /// Uses NodeService.create_node() to ensure NodeCreated event emission.
    /// The pattern is "read from store, write through NodeService".
    async fn create_collection(&self, name: &str) -> Result<Node, NodeServiceError> {
        let validated_name = validate_collection_name(name)?;

        // Deterministic id from the normalized name so independent imports / devices
        // converge on ONE collection node per name instead of minting a fresh random
        // UUID each time (the duplicate-collections bug), and a concurrent
        // double-create resolves to the same node.
        let node_id = deterministic_collection_id(&validated_name);

        // Get-or-create: reuse an existing node with this id (a prior create, a synced
        // copy, or a race winner) rather than erroring on the duplicate id.
        if let Some(existing) = self.node_service.get_node(&node_id).await? {
            return Ok(existing);
        }

        // Use NodeService for node creation (emits NodeCreated event)
        let node = Node::new_with_id(
            node_id.clone(),
            "collection".to_string(),
            validated_name,
            json!({}),
        );
        if let Err(e) = self.node_service.create_node(node).await {
            // A concurrent create of the same deterministic id may have won the race —
            // reuse it rather than failing the import.
            if let Some(existing) = self.node_service.get_node(&node_id).await? {
                return Ok(existing);
            }
            return Err(e);
        }

        // Fetch and return the created node
        self.node_service
            .get_node(&node_id)
            .await?
            .ok_or_else(|| NodeServiceError::node_not_found(&node_id))
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

        // Delegate to NodeService for event emission (unified method)
        self.node_service
            .create_relationship(node_id, "member_of", &resolved.leaf.id, json!({}))
            .await?;

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

        let is_collection = self
            .store
            .type_is_a(
                &collection_node.node_type,
                crate::models::CoreNodeType::Collection,
            )
            .await
            .map_err(|e| db_error(e, "Failed to resolve the collection's type"))?;
        if !is_collection {
            return Err(NodeServiceError::InvalidCollectionPath(format!(
                "Cannot add member to non-collection node: '{}' has type '{}'",
                collection_id, collection_node.node_type
            )));
        }

        // Delegate to NodeService for event emission (unified method)
        self.node_service
            .create_relationship(node_id, "member_of", collection_id, json!({}))
            .await?;

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
        // Delegate to NodeService for event emission (unified method)
        self.node_service
            .delete_relationship(node_id, "member_of", collection_id)
            .await?;

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
    /// Returns full Node structs for all members of the collection.
    /// Single query that traverses the member_of relationship.
    pub async fn get_collection_members(
        &self,
        collection_id: &str,
    ) -> Result<Vec<Node>, NodeServiceError> {
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

    /// Every collection's name and description, sorted by name.
    ///
    /// The description is `None` when the collection has none. This is what
    /// the agent's workspace context lists.
    pub async fn get_all_collection_descriptions(
        &self,
    ) -> Result<Vec<(String, Option<String>)>, NodeServiceError> {
        self.store
            .get_all_collection_descriptions()
            .await
            .map_err(|e| db_error(e, "Failed to get all collection descriptions"))
    }

    /// Get all collections with their member counts and parent collection IDs in a single query
    ///
    /// Uses batch queries to avoid N+1 pattern when fetching collections for UI.
    /// Each collection is returned with its member count and parent IDs pre-computed.
    ///
    /// # Returns
    ///
    /// Vec of (Node, member_count, parent_collection_ids) tuples for all collection nodes
    pub async fn get_all_collections_with_counts(
        &self,
    ) -> Result<Vec<(Node, usize, Vec<String>)>, NodeServiceError> {
        self.store
            .get_all_collections_with_member_counts()
            .await
            .map_err(|e| db_error(e, "Failed to get collections with member counts"))
    }

    /// Resolve multiple collection paths in a single pass
    ///
    /// This is an optimized version of `resolve_path` for bulk import operations.
    /// Instead of resolving paths one at a time (with repeated DB lookups for shared
    /// prefixes), this method:
    ///
    /// 1. Parses all paths to extract unique collection names
    /// 2. Batch-fetches all existing collections in ONE query
    /// 3. Creates missing collections in hierarchy order
    /// 4. Returns a map from path → leaf collection ID
    ///
    /// # Performance
    ///
    /// For 100 files with paths like "Architecture:Core", "Architecture:Components":
    /// - Before: 100 separate resolve_path calls, each querying for "Architecture"
    /// - After: 1 batch query for all unique collection names, then create missing
    ///
    /// # Arguments
    ///
    /// * `paths` - Collection paths to resolve (e.g., ["hr:policy", "hr:vacation", "engineering"])
    ///
    /// # Returns
    ///
    /// HashMap from original path string to the leaf collection ID
    ///
    /// # Example
    ///
    /// ```ignore
    /// let paths = vec!["hr:policy", "hr:vacation", "engineering"];
    /// let resolved = service.bulk_resolve_collections(&paths).await?;
    /// // resolved["hr:policy"] = "collection_id_1"
    /// // resolved["hr:vacation"] = "collection_id_2"
    /// // resolved["engineering"] = "collection_id_3"
    /// ```
    pub async fn bulk_resolve_collections(
        &self,
        paths: &[String],
    ) -> Result<std::collections::HashMap<String, String>, NodeServiceError> {
        use std::collections::{HashMap, HashSet};

        if paths.is_empty() {
            return Ok(HashMap::new());
        }

        // Phase 1: Parse all paths and collect unique collection names
        let mut parsed_paths: Vec<CollectionPath> = Vec::with_capacity(paths.len());
        let mut all_segment_names: HashSet<String> = HashSet::new();

        for path in paths {
            let parsed = parse_collection_path(path)?;
            for segment in &parsed.segments {
                all_segment_names.insert(segment.name.clone());
            }
            parsed_paths.push(parsed);
        }

        // Phase 2: Batch fetch ALL existing collections at once
        let segment_names_vec: Vec<String> = all_segment_names.into_iter().collect();
        let existing_collections = self
            .store
            .get_collections_by_names(&segment_names_vec)
            .await
            .map_err(|e| db_error(e, "Failed to batch fetch collections for bulk resolve"))?;

        // Phase 3: Build collection ID cache (normalized_name → id)
        // Start with existing collections, add newly created ones as we go
        let mut collection_id_cache: HashMap<String, String> = existing_collections
            .iter()
            .map(|(norm_name, node)| (norm_name.clone(), node.id.clone()))
            .collect();

        // Phase 4: Process each path, creating missing collections and hierarchy
        // Track which paths we've already resolved to avoid duplicate work
        let mut path_to_leaf_id: HashMap<String, String> = HashMap::new();

        for parsed in &parsed_paths {
            let mut previous_collection_id: Option<String> = None;

            for segment in &parsed.segments {
                let norm_name = &segment.normalized_name;

                // Check if we already have this collection (from existing or created this session)
                let collection_id = if let Some(id) = collection_id_cache.get(norm_name) {
                    id.clone()
                } else {
                    // Create new collection
                    let new_node = self.create_collection(&segment.name).await?;
                    let new_id = new_node.id.clone();
                    collection_id_cache.insert(norm_name.clone(), new_id.clone());
                    new_id
                };

                // Create hierarchy relationship if there's a parent
                // (idempotent - won't duplicate if already exists)
                if let Some(parent_id) = &previous_collection_id {
                    self.node_service
                        .create_relationship(&collection_id, "member_of", parent_id, json!({}))
                        .await
                        .map_err(|e| {
                            NodeServiceError::query_failed(format!(
                                "Failed to create collection hierarchy in bulk resolve: {}",
                                e
                            ))
                        })?;
                }

                previous_collection_id = Some(collection_id);
            }

            // Map the original path to the leaf collection ID
            if let Some(leaf_id) = previous_collection_id {
                path_to_leaf_id.insert(parsed.original.clone(), leaf_id);
            }
        }

        Ok(path_to_leaf_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

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

    // ---- deterministic collection ids (name-derived, dedupes across imports/devices) ----

    #[test]
    fn deterministic_collection_id_is_stable_and_case_insensitive() {
        let a = deterministic_collection_id("Architecture");
        assert_eq!(
            a,
            deterministic_collection_id("architecture"),
            "case variants converge"
        );
        assert_eq!(
            a,
            deterministic_collection_id("  Architecture  "),
            "surrounding whitespace is trimmed"
        );
        assert_ne!(
            a,
            deterministic_collection_id("Decisions"),
            "distinct names → distinct ids"
        );
        assert!(uuid::Uuid::parse_str(&a).is_ok(), "id is a valid UUID");
    }

    async fn test_node_service() -> (crate::services::NodeService, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().unwrap();
        let mut store = Arc::new(SqliteStore::new(dir.path().join("db")).await.unwrap());
        let ns = crate::services::NodeService::new(&mut store).await.unwrap();
        (ns, dir)
    }

    /// `member_of_edges` as a set, because the query's order is unspecified.
    async fn member_of_edge_set(
        ns: &crate::services::NodeService,
        min_memberships: usize,
        exclude_member_types: &[&str],
    ) -> HashSet<(String, String)> {
        ns.store()
            .member_of_edges(min_memberships, exclude_member_types)
            .await
            .unwrap()
            .into_iter()
            .collect()
    }

    /// The core of the dedup fix: two INDEPENDENT stores (two devices / re-import
    /// runs) creating the same-named collection converge on ONE id, so sync upserts
    /// them into a single node instead of accumulating duplicates.
    #[tokio::test]
    async fn create_collection_id_converges_across_independent_stores() {
        let (ns_a, _a) = test_node_service().await;
        let (ns_b, _b) = test_node_service().await;
        let a = CollectionService::new(ns_a.store(), &ns_a)
            .resolve_path("Architecture")
            .await
            .unwrap();
        let b = CollectionService::new(ns_b.store(), &ns_b)
            .resolve_path("Architecture")
            .await
            .unwrap();
        assert_eq!(
            a.leaf.id, b.leaf.id,
            "independent stores mint the same collection id"
        );
        assert_eq!(a.leaf.id, deterministic_collection_id("Architecture"));
    }

    /// Re-resolving a path (and paths that share a segment) never creates duplicate
    /// collection nodes — get-or-create by deterministic id is idempotent.
    #[tokio::test]
    async fn resolve_path_is_idempotent_no_duplicate_collections() {
        let (ns, _dir) = test_node_service().await;
        let svc = CollectionService::new(ns.store(), &ns);

        let r1 = svc.resolve_path("Architecture:Decisions").await.unwrap();
        let r2 = svc.resolve_path("Architecture:Alternatives").await.unwrap();
        let r1b = svc.resolve_path("Architecture:Decisions").await.unwrap();

        assert_eq!(
            r1.collections[0].node.id, r2.collections[0].node.id,
            "the shared 'Architecture' segment is one node"
        );
        assert_eq!(r1.leaf.id, r1b.leaf.id, "re-resolve is idempotent");
        assert_eq!(
            r1.collections[0].node.id,
            deterministic_collection_id("Architecture")
        );

        // A fresh install seeds no collection, so these three are all of them.
        let names: Vec<String> = svc
            .get_all_collection_descriptions()
            .await
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            names,
            ["Alternatives", "Architecture", "Decisions"],
            "no duplicates"
        );
    }

    /// `member_count` must reflect CONTENT members only. The sidebar uses it to
    /// decide whether a collection is empty (and to prune empty ones), so it has
    /// to agree with what the collection viewer actually lists — user-authored
    /// content, never the system/roster members the viewer filters out. The
    /// seeded demo collections held only a `person` roster and so wrongly counted
    /// as non-empty, then opened blank. Here a decorative `horizontal-line` stands
    /// in for such a non-content member: it must NOT inflate the count.
    #[tokio::test]
    async fn member_count_excludes_non_content_members() {
        let (ns, _dir) = test_node_service().await;
        let svc = CollectionService::new(ns.store(), &ns);

        let research = svc.resolve_path("Research").await.unwrap().leaf.id;

        // One genuine content node and one non-content member, both member_of
        // "Research".
        let text_id = ns
            .create_node(Node::new(
                "text".to_string(),
                "A research note".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        let divider_id = ns
            .create_node(Node::new(
                "horizontal-line".to_string(),
                "---".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        svc.add_to_collection(&text_id, &research).await.unwrap();
        svc.add_to_collection(&divider_id, &research).await.unwrap();

        let counts = svc.get_all_collections_with_counts().await.unwrap();
        let (_, research_count, _) = counts
            .iter()
            .find(|(node, _, _)| node.id == research)
            .expect("Research collection present in listing");

        assert_eq!(
            *research_count, 1,
            "only the text node counts; the horizontal-line member is excluded"
        );
    }

    /// With a threshold of two, `member_of_edges` returns every edge of a node
    /// that is a member of more than one collection and none of the edges of a
    /// node that is a member of only one.
    #[tokio::test]
    async fn member_of_edges_with_threshold_two_skips_single_memberships() {
        let (ns, _dir) = test_node_service().await;
        let svc = CollectionService::new(ns.store(), &ns);
        let default = svc.resolve_path("Default").await.unwrap().leaf.id;
        let topic = svc.resolve_path("Architecture").await.unwrap().leaf.id;

        // Multi-membership node: member_of BOTH collections.
        let multi = ns
            .create_node(Node::new(
                "text".to_string(),
                "multi".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        svc.add_to_collection(&multi, &default).await.unwrap();
        svc.add_to_collection(&multi, &topic).await.unwrap();
        // Single-membership node: member_of only one collection.
        let single = ns
            .create_node(Node::new(
                "text".to_string(),
                "single".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        svc.add_to_collection(&single, &default).await.unwrap();

        let edges = member_of_edge_set(&ns, 2, &[]).await;
        let expected: HashSet<(String, String)> = [
            (multi.clone(), default.clone()),
            (multi.clone(), topic.clone()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            edges, expected,
            "only the multi-membership node's two edges, not the single node's"
        );
    }

    /// `exclude_member_types` drops every edge whose member has a listed
    /// `node_type` and leaves all other edges alone. The exclusion is applied
    /// to the member, not the collection, and it takes any number of types.
    #[tokio::test]
    async fn member_of_edges_excludes_listed_member_types() {
        let (ns, _dir) = test_node_service().await;
        let svc = CollectionService::new(ns.store(), &ns);
        let default = svc.resolve_path("Default").await.unwrap().leaf.id;
        let topic = svc.resolve_path("Architecture").await.unwrap().leaf.id;

        let person = ns
            .get_local_person()
            .await
            .unwrap()
            .expect("the seeded local person");
        svc.add_to_collection(&person.id, &default).await.unwrap();
        svc.add_to_collection(&person.id, &topic).await.unwrap();

        let content = ns
            .create_node(Node::new(
                "text".to_string(),
                "content".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        svc.add_to_collection(&content, &default).await.unwrap();
        svc.add_to_collection(&content, &topic).await.unwrap();

        let person_edges: HashSet<(String, String)> = [
            (person.id.clone(), default.clone()),
            (person.id.clone(), topic.clone()),
        ]
        .into_iter()
        .collect();
        let content_edges: HashSet<(String, String)> = [
            (content.clone(), default.clone()),
            (content.clone(), topic.clone()),
        ]
        .into_iter()
        .collect();

        let unfiltered = member_of_edge_set(&ns, 2, &[]).await;
        assert!(
            person_edges.is_subset(&unfiltered) && content_edges.is_subset(&unfiltered),
            "with nothing excluded both nodes' edges appear: {unfiltered:?}"
        );

        let without_persons = member_of_edge_set(&ns, 2, &["person"]).await;
        assert!(
            without_persons.is_disjoint(&person_edges),
            "the person's edges are absent when `person` is excluded: {without_persons:?}"
        );
        assert!(
            content_edges.is_subset(&without_persons),
            "the content node's edges remain: {without_persons:?}"
        );
        let expected: HashSet<(String, String)> = unfiltered
            .iter()
            .filter(|(member, _)| member != &person.id)
            .cloned()
            .collect();
        assert_eq!(
            without_persons, expected,
            "excluding `person` removes exactly the person's edges"
        );

        assert_eq!(
            member_of_edge_set(&ns, 2, &["not-a-node-type"]).await,
            unfiltered,
            "an unknown type excludes nothing"
        );

        let without_persons_or_text = member_of_edge_set(&ns, 2, &["person", "text"]).await;
        assert!(
            without_persons_or_text.is_disjoint(&person_edges)
                && without_persons_or_text.is_disjoint(&content_edges),
            "every listed type is excluded, not only the first: {without_persons_or_text:?}"
        );
    }

    /// A threshold below one behaves as one: `(0, ..)` and `(1, ..)` return the
    /// same edges, and those include the edge of a node with a single
    /// membership.
    #[tokio::test]
    async fn member_of_edges_threshold_below_one_behaves_as_one() {
        let (ns, _dir) = test_node_service().await;
        let svc = CollectionService::new(ns.store(), &ns);
        let default = svc.resolve_path("Default").await.unwrap().leaf.id;

        let single = ns
            .create_node(Node::new(
                "text".to_string(),
                "single".to_string(),
                json!({}),
            ))
            .await
            .unwrap();
        svc.add_to_collection(&single, &default).await.unwrap();

        let at_zero = member_of_edge_set(&ns, 0, &[]).await;
        let at_one = member_of_edge_set(&ns, 1, &[]).await;

        assert_eq!(at_zero, at_one, "a threshold of 0 behaves as 1");
        assert!(
            at_one.contains(&(single, default)),
            "a threshold of 1 includes a single-membership node's edge: {at_one:?}"
        );
    }
}
