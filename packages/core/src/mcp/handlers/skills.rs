//! MCP handler for skill discovery.
//!
//! Exposes `find_skills` as an MCP tool for external agents (via ACP) to
//! discover available skills in the NodeSpace knowledge graph.
//!
//! Issue #1051, ADR-030 Phase 4.

use crate::mcp::MCPError;
use crate::services::{NodeEmbeddingService, NodeService};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct FindSkillsParams {
    query: String,
    limit: Option<usize>,
}

/// Handle find_skills tool call.
///
/// Searches for skill nodes via semantic search and returns results
/// with confidence-based detail levels:
/// - > 0.8: full skill with tools and description
/// - 0.6-0.8: description only (let agent decide)
/// - < 0.6: excluded from results
pub async fn handle_find_skills(
    _node_service: &Arc<NodeService>,
    embedding_service: &Arc<NodeEmbeddingService>,
    arguments: Value,
) -> Result<Value, MCPError> {
    let params: FindSkillsParams = serde_json::from_value(arguments)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    let limit = params.limit.unwrap_or(3).min(10);

    // Semantic search for skill nodes
    let results = embedding_service
        .semantic_search(&params.query, limit * 2, 0.3)
        .await
        .map_err(|e| MCPError::internal_error(format!("Skill search failed: {}", e)))?;

    // Filter to skill nodes only
    let skill_results: Vec<_> = results
        .into_iter()
        .filter(|r| {
            r.node
                .as_ref()
                .map(|n| n.node_type == "skill")
                .unwrap_or(false)
        })
        .take(limit)
        .collect();

    let mut skills = Vec::new();
    for result in &skill_results {
        if let Some(ref node) = result.node {
            let confidence = result.max_similarity;
            let description = node
                .properties
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let tool_whitelist = node
                .properties
                .get("tool_whitelist")
                .cloned()
                .unwrap_or(json!([]));

            if confidence > 0.8 {
                skills.push(json!({
                    "id": node.id,
                    "name": node.content,
                    "description": description,
                    "confidence": format!("{:.2}", confidence),
                    "tools": tool_whitelist,
                    "recommendation": "Use this skill's tools for your task"
                }));
            } else if confidence > 0.6 {
                skills.push(json!({
                    "id": node.id,
                    "name": node.content,
                    "description": description,
                    "confidence": format!("{:.2}", confidence),
                    "recommendation": "May be relevant - review before adopting"
                }));
            }
        }
    }

    tracing::info!(
        query = %params.query,
        results_found = skill_results.len(),
        skills_returned = skills.len(),
        "find_skills MCP tool executed"
    );

    if skills.is_empty() {
        Ok(json!({
            "message": "No matching skills found. Proceed with general capabilities.",
            "query": params.query
        }))
    } else {
        Ok(json!({
            "count": skills.len(),
            "skills": skills,
            "query": params.query
        }))
    }
}
