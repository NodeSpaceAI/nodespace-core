//! MCP handler for skill discovery.
//!
//! Exposes `find_skills` as an MCP tool for external agents (via ACP) to
//! discover available skills in the NodeSpace knowledge graph.
//!
//! Issue #1051, ADR-030 Phase 4.

use crate::mcp::MCPError;
use crate::ops::skill_ops;
use crate::services::{NodeEmbeddingService, NodeService};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct FindSkillsParams {
    query: String,
    limit: Option<usize>,
}

/// Handle find_skills tool call via shared skill_ops layer.
pub async fn handle_find_skills(
    _node_service: &Arc<NodeService>,
    embedding_service: &Arc<NodeEmbeddingService>,
    arguments: Value,
) -> Result<Value, MCPError> {
    let params: FindSkillsParams = serde_json::from_value(arguments)
        .map_err(|e| MCPError::invalid_params(format!("Invalid parameters: {}", e)))?;

    let output = skill_ops::find_skills(
        embedding_service,
        skill_ops::FindSkillsInput {
            query: params.query,
            limit: params.limit,
        },
    )
    .await
    .map_err(|e| MCPError::internal_error(e.to_string()))?;

    if output.skills.is_empty() {
        Ok(json!({
            "message": "No matching skills found. Proceed with general capabilities.",
            "query": output.query
        }))
    } else {
        Ok(json!({
            "count": output.skills.len(),
            "skills": output.skills,
            "query": output.query
        }))
    }
}
