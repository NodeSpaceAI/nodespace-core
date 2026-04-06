//! Skill discovery operations.
//!
//! Shared logic for `find_skills` used by both the local agent tool and the
//! MCP handler. Searches for skill nodes via semantic search and returns
//! results with confidence-based detail levels.
//!
//! Issue #1051, ADR-030 Phase 4.

use crate::services::NodeEmbeddingService;
use serde_json::{json, Value};
use std::sync::Arc;

use super::OpsError;

/// High confidence threshold: full skill details with tool whitelist.
pub const SKILL_HIGH_CONFIDENCE: f64 = 0.8;

/// Medium confidence threshold: description only (let agent decide).
pub const SKILL_MEDIUM_CONFIDENCE: f64 = 0.6;

/// Minimum similarity threshold for skill search.
const SKILL_SEARCH_THRESHOLD: f32 = 0.3;

/// Maximum limit for skill search results.
const MAX_SKILL_LIMIT: usize = 10;

/// Input for find_skills operation.
#[derive(Debug)]
pub struct FindSkillsInput {
    pub query: String,
    pub limit: Option<usize>,
}

/// Output for find_skills operation.
#[derive(Debug)]
pub struct FindSkillsOutput {
    pub skills: Vec<Value>,
    pub query: String,
    pub total_results: usize,
}

/// Search for skill nodes via semantic search with confidence-based detail levels.
///
/// Returns skills with varying detail based on confidence:
/// - Above `SKILL_HIGH_CONFIDENCE` (0.8): full skill with tool whitelist
/// - Between `SKILL_MEDIUM_CONFIDENCE` (0.6) and high: description only
/// - Below medium: excluded from results
pub async fn find_skills(
    embedding_service: &Arc<NodeEmbeddingService>,
    input: FindSkillsInput,
) -> Result<FindSkillsOutput, OpsError> {
    let limit = input.limit.unwrap_or(3).min(MAX_SKILL_LIMIT);

    // Over-fetch and filter to skill nodes
    let results = embedding_service
        .semantic_search(&input.query, limit * 3, SKILL_SEARCH_THRESHOLD)
        .await
        .map_err(|e| OpsError::Internal(format!("Skill search failed: {}", e)))?;

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

    let total_results = skill_results.len();
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

            if confidence > SKILL_HIGH_CONFIDENCE {
                skills.push(json!({
                    "id": node.id,
                    "name": node.content,
                    "description": description,
                    "confidence": format!("{:.2}", confidence),
                    "tools": tool_whitelist,
                    "recommendation": "Use this skill's tools for your task"
                }));
            } else if confidence > SKILL_MEDIUM_CONFIDENCE {
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
        query = %input.query,
        results_found = total_results,
        skills_returned = skills.len(),
        top_score = skill_results.first().map(|r| r.max_similarity).unwrap_or(0.0),
        "find_skills executed"
    );

    Ok(FindSkillsOutput {
        skills,
        query: input.query,
        total_results,
    })
}
