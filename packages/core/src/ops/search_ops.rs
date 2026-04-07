//! Search Operations
//!
//! Semantic search orchestration extracted from MCP handlers.
//! Handles collection resolution, scope filtering, lifecycle filtering,
//! over-fetching, and optional markdown inlining.

use crate::models::Node;
use crate::ops::OpsError;
use crate::services::{
    CollectionService, NodeEmbeddingService, NodeService, NodeServiceError, SearchScope,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Maximum depth for markdown tree traversal
const MARKDOWN_MAX_DEPTH: usize = 20;

// ============================================================================
// Input / Output types
// ============================================================================

#[derive(Debug)]
pub struct SearchSemanticInput {
    pub query: String,
    pub threshold: Option<f32>,
    pub limit: Option<usize>,
    pub collection_id: Option<String>,
    pub collection: Option<String>,
    pub exclude_collections: Option<Vec<String>>,
    pub include_markdown: Option<usize>,
    pub include_archived: Option<bool>,
    pub scope: Option<String>,
}

#[derive(Debug)]
pub struct SearchSemanticOutput {
    pub nodes: Vec<Value>,
    pub count: usize,
    pub query: String,
    pub threshold: f32,
    pub collection_id: Option<String>,
    pub include_markdown: usize,
    pub include_archived: bool,
    pub scope: String,
}

// ============================================================================
// Helpers
// ============================================================================

/// Recursively build markdown from a node tree
fn build_markdown_recursive(
    node: &Node,
    node_map: &HashMap<String, Node>,
    adjacency_list: &HashMap<String, Vec<String>>,
    output: &mut String,
    depth: usize,
    max_depth: usize,
) {
    if depth > max_depth {
        return;
    }

    match node.node_type.as_str() {
        "header" => {
            output.push_str(&node.content);
            output.push_str("\n\n");
        }
        "text" => {
            output.push_str(&node.content);
            output.push_str("\n\n");
        }
        "task" => {
            let status = node
                .properties
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("todo");
            let checkbox = if status == "done" { "[x]" } else { "[ ]" };
            output.push_str(&format!("- {} {}\n", checkbox, node.content));
        }
        "code-block" => {
            let language = node
                .properties
                .get("language")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            output.push_str(&format!("```{}\n{}\n```\n\n", language, node.content));
        }
        "quote-block" => {
            for line in node.content.lines() {
                output.push_str(&format!("> {}\n", line));
            }
            output.push('\n');
        }
        "ordered-list" => {
            output.push_str(&format!("1. {}\n", node.content));
        }
        _ => {
            output.push_str(&node.content);
            output.push_str("\n\n");
        }
    }

    if let Some(child_ids) = adjacency_list.get(&node.id) {
        for child_id in child_ids {
            if let Some(child) = node_map.get(child_id) {
                build_markdown_recursive(
                    child,
                    node_map,
                    adjacency_list,
                    output,
                    depth + 1,
                    max_depth,
                );
            }
        }
    }
}

fn parse_scope(scope: Option<&str>) -> Result<SearchScope, OpsError> {
    match scope {
        Some("conversations") => Ok(SearchScope::Conversations),
        Some("everything") => Ok(SearchScope::Everything),
        Some("knowledge") | None => Ok(SearchScope::Knowledge),
        Some(unknown) => Err(OpsError::InvalidParams(format!(
            "Invalid scope '{}'. Valid values: knowledge, conversations, everything",
            unknown
        ))),
    }
}

// ============================================================================
// Operation
// ============================================================================

/// Semantic search with collection resolution, scope/lifecycle filtering,
/// over-fetching, and optional markdown inlining for top results.
pub async fn search_semantic(
    node_service: &Arc<NodeService>,
    embedding_service: &Arc<NodeEmbeddingService>,
    input: SearchSemanticInput,
) -> Result<SearchSemanticOutput, OpsError> {
    // Apply defaults
    let threshold = input.threshold.unwrap_or(0.7);
    let limit = input.limit.unwrap_or(20);
    let include_markdown = input.include_markdown.unwrap_or(1).min(5);
    let include_archived = input.include_archived.unwrap_or(false);
    let scope = parse_scope(input.scope.as_deref())?;

    // Validate
    if !(0.0..=1.0).contains(&threshold) {
        return Err(OpsError::InvalidParams(
            "threshold must be between 0.0 and 1.0".to_string(),
        ));
    }
    if limit > 1000 {
        return Err(OpsError::InvalidParams(
            "limit cannot exceed 1000".to_string(),
        ));
    }
    if input.query.trim().is_empty() {
        return Err(OpsError::InvalidParams(
            "query cannot be empty or whitespace".to_string(),
        ));
    }

    // Resolve collection include filter
    let (collection_id, collection_member_ids): (Option<String>, Option<HashSet<String>>) =
        if let Some(path) = &input.collection {
            let collection_service =
                CollectionService::new(embedding_service.store(), node_service);
            match collection_service.resolve_path(path).await {
                Ok(resolved) => {
                    let coll_id = resolved.leaf_id().to_string();
                    let members = collection_service
                        .get_collection_members(&coll_id)
                        .await
                        .map_err(|e| {
                            OpsError::Internal(format!("Failed to get collection members: {}", e))
                        })?;
                    (
                        Some(coll_id),
                        Some(members.into_iter().map(|n| n.id).collect()),
                    )
                }
                Err(NodeServiceError::CollectionNotFound(_)) => {
                    return Ok(SearchSemanticOutput {
                        nodes: vec![],
                        count: 0,
                        query: input.query,
                        threshold,
                        collection_id: None,
                        include_markdown,
                        include_archived,
                        scope: format!("{:?}", scope),
                    });
                }
                Err(e) => {
                    return Err(OpsError::Internal(format!(
                        "Failed to resolve collection path: {}",
                        e
                    )));
                }
            }
        } else if let Some(coll_id) = &input.collection_id {
            let collection_service =
                CollectionService::new(embedding_service.store(), node_service);
            let members = collection_service
                .get_collection_members(coll_id)
                .await
                .map_err(|e| {
                    OpsError::Internal(format!("Failed to get collection members: {}", e))
                })?;
            (
                Some(coll_id.clone()),
                Some(members.into_iter().map(|n| n.id).collect()),
            )
        } else {
            (None, None)
        };

    // Resolve excluded collections
    let excluded_node_ids: HashSet<String> = if let Some(exclude_paths) = &input.exclude_collections
    {
        let collection_service = CollectionService::new(embedding_service.store(), node_service);
        let mut excluded = HashSet::new();
        for path in exclude_paths {
            match collection_service.resolve_path(path).await {
                Ok(resolved) => {
                    let coll_id = resolved.leaf_id().to_string();
                    if let Ok(members) = collection_service.get_collection_members(&coll_id).await {
                        excluded.extend(members.into_iter().map(|n| n.id));
                    }
                }
                Err(NodeServiceError::CollectionNotFound(_)) => {
                    tracing::debug!("Excluded collection '{}' not found, skipping", path);
                }
                Err(e) => {
                    tracing::warn!("Failed to resolve excluded collection '{}': {}", path, e);
                }
            }
        }
        excluded
    } else {
        HashSet::new()
    };

    tracing::info!(
        "Semantic search for: '{}' (scope: {:?})",
        input.query,
        scope
    );

    // Over-fetch when post-filtering is needed
    let scope_filters = !matches!(scope, SearchScope::Everything);
    let has_post_filters = collection_member_ids.is_some()
        || !excluded_node_ids.is_empty()
        || !include_archived
        || scope_filters;
    let effective_limit = if has_post_filters { limit * 3 } else { limit };

    let results = embedding_service
        .semantic_search_nodes(&input.query, effective_limit, threshold, None)
        .await
        .map_err(|e| {
            let err_msg = e.to_string();
            if err_msg.contains("not initialized") || err_msg.contains("not available") {
                OpsError::Internal("Embedding service not ready".to_string())
            } else if err_msg.contains("no embeddings") || err_msg.contains("not found") {
                OpsError::InvalidParams(
                    "No content available for semantic search. Try adding content first."
                        .to_string(),
                )
            } else if err_msg.contains("database") || err_msg.contains("Database") {
                OpsError::Internal(format!("Database error during search: {}", e))
            } else {
                OpsError::Internal(format!("Search failed: {}", e))
            }
        })?;

    // Apply filters
    let filtered_results: Vec<_> = results
        .into_iter()
        .filter(|(node, _)| {
            if !NodeEmbeddingService::matches_scope(&node.node_type, &scope) {
                return false;
            }
            let status = &node.lifecycle_status;
            if status == "deleted" {
                return false;
            }
            if status == "archived" && !include_archived {
                return false;
            }
            if let Some(ref member_ids) = collection_member_ids {
                if !member_ids.contains(&node.id) {
                    return false;
                }
            }
            if excluded_node_ids.contains(&node.id) {
                return false;
            }
            true
        })
        .take(limit)
        .collect();

    // Fetch markdown for top N results
    let mut markdown_contents: HashMap<String, String> = HashMap::new();
    if include_markdown > 0 {
        for (node, _) in filtered_results.iter().take(include_markdown) {
            if let Ok((Some(root_node), node_map, adjacency_list)) =
                node_service.get_subtree_data(&node.id).await
            {
                let mut markdown = String::new();
                markdown.push_str(&root_node.content);
                markdown.push_str("\n\n");

                if let Some(child_ids) = adjacency_list.get(&root_node.id) {
                    for child_id in child_ids {
                        if let Some(child) = node_map.get(child_id) {
                            build_markdown_recursive(
                                child,
                                &node_map,
                                &adjacency_list,
                                &mut markdown,
                                0,
                                MARKDOWN_MAX_DEPTH,
                            );
                        }
                    }
                }

                markdown_contents.insert(node.id.clone(), markdown.trim().to_string());
            }
        }
    }

    // Build output
    let nodes: Vec<Value> = filtered_results
        .iter()
        .enumerate()
        .map(|(idx, (node, similarity))| {
            let mut node_json = json!({
                "id": node.id,
                "nodeType": node.node_type,
                "content": node.content,
                "title": node.title,
                "version": node.version,
                "createdAt": node.created_at,
                "modifiedAt": node.modified_at,
                "properties": node.properties,
                "similarity": similarity
            });

            if idx < include_markdown {
                if let Some(markdown) = markdown_contents.get(&node.id) {
                    node_json["markdown"] = json!(markdown);
                }
            }

            node_json
        })
        .collect();

    let count = nodes.len();

    Ok(SearchSemanticOutput {
        nodes,
        count,
        query: input.query,
        threshold,
        collection_id,
        include_markdown,
        include_archived,
        scope: format!("{:?}", scope),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_scope_valid() {
        assert!(matches!(parse_scope(None), Ok(SearchScope::Knowledge)));
        assert!(matches!(
            parse_scope(Some("knowledge")),
            Ok(SearchScope::Knowledge)
        ));
        assert!(matches!(
            parse_scope(Some("conversations")),
            Ok(SearchScope::Conversations)
        ));
        assert!(matches!(
            parse_scope(Some("everything")),
            Ok(SearchScope::Everything)
        ));
    }

    #[test]
    fn test_parse_scope_invalid() {
        let err = parse_scope(Some("bogus")).unwrap_err();
        assert!(matches!(err, OpsError::InvalidParams(_)));
    }
}
