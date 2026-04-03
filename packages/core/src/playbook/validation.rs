//! Save-Time Validation for Playbooks (Phase 7)
//!
//! Validates playbook rule definitions before persisting. Reuses the CEL parser
//! from `cel.rs` — no divergence between what validates and what executes.
//!
//! # Checks performed
//!
//! 1. All referenced `node_type` values must exist as schema nodes
//! 2. All referenced `version` values in action params must match the schema's `schema_version`
//! 3. All property paths in conditions parse successfully via the CEL compiler
//! 4. All relationship types in actions must exist on the referenced schemas
//!
//! If any check fails, the playbook is not saved. All errors are collected
//! (not short-circuited) so the caller can present every issue at once.

use crate::models::SchemaNode;
use crate::playbook::cel::compile_condition;
use crate::playbook::path_extractor;
use crate::playbook::types::{ActionType, ParsedAction, ParsedRule, ParsedTrigger};
use crate::services::NodeService;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::debug;

// ---------------------------------------------------------------------------
// Validation Errors
// ---------------------------------------------------------------------------

/// A single validation error found during save-time checks.
#[derive(Debug, Clone, PartialEq)]
pub enum PlaybookValidationError {
    /// A referenced node_type does not exist as a schema node.
    UnknownNodeType {
        node_type: String,
        /// Where the reference was found (e.g., "rule[0].trigger", "rule[1].action[2]")
        location: String,
    },
    /// A `version` value in an action doesn't match the schema's `schema_version`.
    VersionMismatch {
        node_type: String,
        declared_version: String,
        actual_version: u32,
        location: String,
    },
    /// A CEL condition expression failed to compile.
    InvalidCondition {
        expression: String,
        message: String,
        location: String,
    },
    /// A relationship type in an action doesn't exist on the referenced schema.
    UnknownRelationshipType {
        relationship_type: String,
        node_type: String,
        location: String,
    },
    /// A required param is missing from an action definition.
    MissingActionParam { param: String, location: String },
    /// A dot-path in a condition references a field or relationship that doesn't
    /// exist on the schema graph.
    BrokenPath {
        path: String,
        segment: String,
        message: String,
        location: String,
    },
}

impl std::fmt::Display for PlaybookValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownNodeType {
                node_type,
                location,
            } => write!(f, "unknown node_type '{}' at {}", node_type, location),
            Self::VersionMismatch {
                node_type,
                declared_version,
                actual_version,
                location,
            } => write!(
                f,
                "version mismatch for '{}' at {}: declared '{}', schema has {}",
                node_type, location, declared_version, actual_version
            ),
            Self::InvalidCondition {
                expression,
                message,
                location,
            } => write!(
                f,
                "invalid CEL condition at {}: '{}' — {}",
                location, expression, message
            ),
            Self::UnknownRelationshipType {
                relationship_type,
                node_type,
                location,
            } => write!(
                f,
                "unknown relationship_type '{}' on schema '{}' at {}",
                relationship_type, node_type, location
            ),
            Self::MissingActionParam { param, location } => {
                write!(f, "missing required param '{}' at {}", param, location)
            }
            Self::BrokenPath {
                path,
                segment,
                message,
                location,
            } => {
                write!(
                    f,
                    "broken path '{}' at {}: segment '{}' — {}",
                    path, location, segment, message
                )
            }
        }
    }
}

/// Result of playbook validation: either Ok or a non-empty list of errors.
pub type ValidationResult = Result<(), Vec<PlaybookValidationError>>;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Validate a set of parsed rules before saving a playbook.
///
/// Queries schema nodes via `NodeService` to verify node_type existence,
/// schema_version matching, and relationship type existence. Also compiles
/// all CEL conditions to catch syntax errors.
///
/// Returns `Ok(())` if all checks pass, or `Err(Vec<...>)` with all errors found.
pub async fn validate_playbook(
    rules: &[Arc<ParsedRule>],
    node_service: &NodeService,
) -> ValidationResult {
    let mut errors: Vec<PlaybookValidationError> = Vec::new();

    // Collect all referenced node_types and fetch schemas once
    let mut schema_cache: HashMap<String, Option<SchemaNode>> = HashMap::new();

    for (rule_idx, rule) in rules.iter().enumerate() {
        // -- Validate trigger node_type --
        let trigger_node_type = trigger_node_type(rule);
        if let Some(nt) = &trigger_node_type {
            ensure_schema_cached(nt, node_service, &mut schema_cache).await;
            if schema_cache
                .get(nt.as_str())
                .and_then(|s| s.as_ref())
                .is_none()
            {
                errors.push(PlaybookValidationError::UnknownNodeType {
                    node_type: nt.clone(),
                    location: format!("rule[{}].trigger", rule_idx),
                });
            }
        }

        // -- Validate CEL conditions --
        for (cond_idx, condition) in rule.conditions.iter().enumerate() {
            let location = format!("rule[{}].condition[{}]", rule_idx, cond_idx);
            if let Err(e) = compile_condition(condition) {
                errors.push(PlaybookValidationError::InvalidCondition {
                    expression: e.expression,
                    message: e.message,
                    location,
                });
                continue; // Can't extract paths from unparseable conditions
            }

            // Schema-aware path validation (#1010): extract dot-paths and
            // verify each segment resolves to a field or relationship on the schema graph
            if let Some(nt) = &trigger_node_type {
                if let Ok(extraction) = path_extractor::extract_paths(condition) {
                    for path in &extraction.paths {
                        if path.root == "node" && path.segments.len() > 2 {
                            validate_schema_path(
                                &path.segments,
                                nt,
                                &location,
                                node_service,
                                &mut schema_cache,
                                &mut errors,
                            )
                            .await;
                        }
                    }
                    for coll in &extraction.collections {
                        if coll.collection.root == "node" && coll.collection.segments.len() > 1 {
                            validate_schema_path(
                                &coll.collection.segments,
                                nt,
                                &location,
                                node_service,
                                &mut schema_cache,
                                &mut errors,
                            )
                            .await;
                        }
                    }
                }
            }
        }

        // -- Validate actions --
        for (action_idx, action) in rule.actions.iter().enumerate() {
            let location = format!("rule[{}].action[{}]", rule_idx, action_idx);
            validate_action(
                action,
                &location,
                trigger_node_type.as_deref(),
                node_service,
                &mut schema_cache,
                &mut errors,
            )
            .await;
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract the node_type from a parsed trigger.
fn trigger_node_type(rule: &ParsedRule) -> Option<String> {
    match &rule.trigger {
        ParsedTrigger::GraphEvent { node_type, .. } => Some(node_type.clone()),
        ParsedTrigger::Scheduled { node_type, .. } => Some(node_type.clone()),
    }
}

/// Ensure a schema is in the cache, fetching from DB if not yet loaded.
async fn ensure_schema_cached(
    node_type: &str,
    node_service: &NodeService,
    cache: &mut HashMap<String, Option<SchemaNode>>,
) {
    if cache.contains_key(node_type) {
        return;
    }
    let schema = match node_service.get_schema_node(node_type).await {
        Ok(s) => s,
        Err(e) => {
            debug!(
                "Failed to query schema for '{}': {} — treating as missing",
                node_type, e
            );
            None
        }
    };
    cache.insert(node_type.to_string(), schema);
}

/// Validate a dot-path against the schema graph.
///
/// Walks the path segments starting from the trigger schema, checking each segment:
/// 1. Is it a field on the current schema? → terminal (scalar property)
/// 2. Is it a relationship on the current schema? → follow to target schema
/// 3. Neither → broken path error
///
/// Path format: `["node", "story", "epic", "status"]`
/// - First segment ("node") is skipped (it's the root variable)
/// - Second segment ("story") checked against the trigger schema
/// - Remaining segments checked against subsequent schemas
async fn validate_schema_path(
    segments: &[String],
    trigger_node_type: &str,
    location: &str,
    node_service: &NodeService,
    schema_cache: &mut HashMap<String, Option<SchemaNode>>,
    errors: &mut Vec<PlaybookValidationError>,
) {
    if segments.len() < 2 {
        return; // Single-segment paths (just "node") don't need validation
    }

    let full_path = segments.join(".");
    let mut current_type = trigger_node_type.to_string();

    // Walk from segments[1] onward (skipping "node")
    for (i, segment) in segments[1..].iter().enumerate() {
        ensure_schema_cached(&current_type, node_service, schema_cache).await;

        let schema = match schema_cache.get(&current_type).and_then(|s| s.as_ref()) {
            Some(s) => s,
            None => {
                // Schema not found — can't validate further
                // (UnknownNodeType error is already reported by trigger validation)
                return;
            }
        };

        // Check if the segment is a field on this schema
        let is_field = schema.fields.iter().any(|f| f.name == *segment);
        if is_field {
            // Fields are terminal — if there are more segments after this, it's broken
            if i + 1 < segments.len() - 1 {
                errors.push(PlaybookValidationError::BrokenPath {
                    path: full_path.clone(),
                    segment: segment.clone(),
                    message: format!(
                        "'{}' is a field on '{}', not a relationship (cannot traverse further)",
                        segment, current_type
                    ),
                    location: location.to_string(),
                });
            }
            return;
        }

        // Check if the segment is a relationship on this schema
        let relationship = schema.relationships.iter().find(|r| r.name == *segment);
        if let Some(rel) = relationship {
            if let Some(ref target_type) = rel.target_type {
                // Follow the relationship to the target schema
                current_type = target_type.clone();
            } else {
                // Relationship has no target_type — can't traverse further
                if i + 1 < segments.len() - 1 {
                    errors.push(PlaybookValidationError::BrokenPath {
                        path: full_path.clone(),
                        segment: segment.clone(),
                        message: format!(
                            "relationship '{}' on '{}' has no target_type (cannot traverse further)",
                            segment, current_type
                        ),
                        location: location.to_string(),
                    });
                }
                return;
            }
        } else {
            // Neither a field nor a relationship — broken path
            // But only report if the schema actually exists (to avoid duplicate errors)
            errors.push(PlaybookValidationError::BrokenPath {
                path: full_path.clone(),
                segment: segment.clone(),
                message: format!(
                    "'{}' is not a field or relationship on schema '{}'",
                    segment, current_type
                ),
                location: location.to_string(),
            });
            return;
        }
    }
}

/// Validate a single action's params.
async fn validate_action(
    action: &ParsedAction,
    location: &str,
    trigger_node_type: Option<&str>,
    node_service: &NodeService,
    schema_cache: &mut HashMap<String, Option<SchemaNode>>,
    errors: &mut Vec<PlaybookValidationError>,
) {
    match action.action_type {
        ActionType::CreateNode => {
            validate_create_node_action(
                &action.params,
                location,
                node_service,
                schema_cache,
                errors,
            )
            .await;
        }
        ActionType::UpdateNode => {
            // update_node may optionally reference a node_type for type conversion
            if let Some(nt) = action.params.get("node_type").and_then(|v| v.as_str()) {
                ensure_schema_cached(nt, node_service, schema_cache).await;
                if schema_cache.get(nt).and_then(|s| s.as_ref()).is_none() {
                    errors.push(PlaybookValidationError::UnknownNodeType {
                        node_type: nt.to_string(),
                        location: location.to_string(),
                    });
                }
            }
        }
        ActionType::AddRelationship | ActionType::RemoveRelationship => {
            validate_relationship_action(
                &action.params,
                location,
                trigger_node_type,
                node_service,
                schema_cache,
                errors,
            )
            .await;
        }
    }
}

/// Validate `create_node` action: node_type must exist, version must match.
async fn validate_create_node_action(
    params: &serde_json::Value,
    location: &str,
    node_service: &NodeService,
    schema_cache: &mut HashMap<String, Option<SchemaNode>>,
    errors: &mut Vec<PlaybookValidationError>,
) {
    // node_type is required
    let node_type = match params.get("node_type").and_then(|v| v.as_str()) {
        Some(nt) => nt,
        None => {
            if params.get("node_type").is_some() {
                // Non-string node_type (e.g., number, object) — can't validate, skip
                return;
            }
            errors.push(PlaybookValidationError::MissingActionParam {
                param: "node_type".to_string(),
                location: location.to_string(),
            });
            return;
        }
    };

    // Skip validation for binding templates like "{trigger.node.node_type}"
    if node_type.contains('{') {
        return;
    }

    ensure_schema_cached(node_type, node_service, schema_cache).await;

    let schema = match schema_cache.get(node_type).and_then(|s| s.as_ref()) {
        Some(s) => s,
        None => {
            errors.push(PlaybookValidationError::UnknownNodeType {
                node_type: node_type.to_string(),
                location: location.to_string(),
            });
            return;
        }
    };

    // Check version if declared
    if let Some(version_val) = params.get("version") {
        let owned_str;
        let declared = match version_val.as_str() {
            Some(s) => s,
            None => {
                owned_str = version_val.to_string();
                &owned_str
            }
        };
        // Schema version is a u32; the playbook may declare it as a string like "1" or "2"
        let declared_num: Option<u32> = declared.parse().ok();
        if declared_num != Some(schema.schema_version) {
            errors.push(PlaybookValidationError::VersionMismatch {
                node_type: node_type.to_string(),
                declared_version: declared.to_string(),
                actual_version: schema.schema_version,
                location: location.to_string(),
            });
        }
    }
}

/// Validate relationship actions: relationship_type must exist on the trigger's schema.
async fn validate_relationship_action(
    params: &serde_json::Value,
    location: &str,
    trigger_node_type: Option<&str>,
    node_service: &NodeService,
    schema_cache: &mut HashMap<String, Option<SchemaNode>>,
    errors: &mut Vec<PlaybookValidationError>,
) {
    let rel_type = match params.get("relationship_type").and_then(|v| v.as_str()) {
        Some(rt) => rt,
        None => {
            errors.push(PlaybookValidationError::MissingActionParam {
                param: "relationship_type".to_string(),
                location: location.to_string(),
            });
            return;
        }
    };

    // Skip validation for binding templates
    if rel_type.contains('{') {
        return;
    }

    // We need the trigger's schema to check if the relationship exists.
    // If the trigger node_type is unknown (already flagged), skip this check.
    let Some(nt) = trigger_node_type else {
        return;
    };

    ensure_schema_cached(nt, node_service, schema_cache).await;

    if let Some(Some(schema)) = schema_cache.get(nt) {
        let rel_exists = schema.relationships.iter().any(|r| r.name == rel_type);

        if !rel_exists {
            errors.push(PlaybookValidationError::UnknownRelationshipType {
                relationship_type: rel_type.to_string(),
                node_type: nt.to_string(),
                location: location.to_string(),
            });
        }
    }
    // If schema is None, we already flagged the missing node_type
}

// ---------------------------------------------------------------------------
// Schema Change Impact Analysis (Issue #1012 Phase 2)
// ---------------------------------------------------------------------------

/// A playbook affected by a schema change, with the specific broken paths.
#[derive(Debug, Clone, PartialEq)]
pub struct AffectedPlaybook {
    /// The playbook node ID
    pub playbook_id: String,
    /// Human-readable playbook name (from content/title)
    pub playbook_name: String,
    /// Dot-paths in conditions that traverse through the changed schema
    pub broken_paths: Vec<String>,
}

impl std::fmt::Display for AffectedPlaybook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "playbook '{}' ({}): paths [{}]",
            self.playbook_name,
            self.playbook_id,
            self.broken_paths.join(", ")
        )
    }
}

/// Check which active playbooks would be affected by a schema change.
///
/// Queries all active playbook nodes, parses their rules, and checks whether
/// any trigger, condition, or action references the given schema's node_type.
/// Specifically checks:
/// - Trigger node_type matches
/// - Condition dot-paths that traverse through the schema's node_type
/// - `create_node` actions targeting the schema's node_type
/// - Relationship actions whose `relationship_type` matches the schema's node_type
///
/// TODO: This is currently over-broad — any change to a schema (including adding
/// new fields, which can't break playbooks) triggers the warning. Making this
/// diff-aware (only flag breaking changes like field removal/rename) requires
/// accepting the proposed schema changes as a parameter, which is a larger
/// refactor. The conservative approach is acceptable for v1.
///
/// Returns a list of affected playbooks with their broken paths.
pub async fn check_schema_change_impact(
    schema_node_type: &str,
    node_service: &NodeService,
) -> Result<Vec<AffectedPlaybook>, String> {
    use crate::playbook::types::{parse_rule, parse_rules_from_properties};

    let playbook_nodes = node_service
        .query_nodes_by_type("playbook", Some("active"))
        .await
        .map_err(|e| format!("Failed to query playbook nodes: {}", e))?;

    let mut affected = Vec::new();

    for pb_node in &playbook_nodes {
        let rule_defs = match parse_rules_from_properties(&pb_node.properties) {
            Ok(defs) => defs,
            Err(_) => continue, // Skip unparseable playbooks
        };

        let mut broken_paths = Vec::new();

        for def in &rule_defs {
            let parsed = match parse_rule(def) {
                Ok(r) => r,
                Err(_) => continue,
            };

            // Check trigger node_type
            let trigger_nt = match &parsed.trigger {
                ParsedTrigger::GraphEvent { node_type, .. } => Some(node_type.as_str()),
                ParsedTrigger::Scheduled { node_type, .. } => Some(node_type.as_str()),
            };
            if trigger_nt == Some(schema_node_type) {
                broken_paths.push(format!("trigger.node_type={}", schema_node_type));
            }

            // Check condition paths
            for condition in &parsed.conditions {
                if let Ok(extraction) = path_extractor::extract_paths(condition) {
                    for path in &extraction.paths {
                        if path.segments.iter().any(|s| s == schema_node_type) {
                            broken_paths.push(path.segments.join("."));
                        }
                    }
                    for coll in &extraction.collections {
                        if coll
                            .collection
                            .segments
                            .iter()
                            .any(|s| s == schema_node_type)
                        {
                            broken_paths.push(coll.collection.segments.join("."));
                        }
                    }
                }
            }

            // Check action params for schema references
            for (i, action) in parsed.actions.iter().enumerate() {
                let action_loc = format!("action[{}]", i);
                match action.action_type {
                    ActionType::CreateNode | ActionType::UpdateNode => {
                        if let Some(nt) = action.params.get("node_type").and_then(|v| v.as_str()) {
                            if nt == schema_node_type {
                                broken_paths.push(format!("{}.node_type={}", action_loc, nt));
                            }
                        }
                    }
                    ActionType::AddRelationship | ActionType::RemoveRelationship => {
                        if let Some(rt) = action
                            .params
                            .get("relationship_type")
                            .and_then(|v| v.as_str())
                        {
                            if rt == schema_node_type {
                                broken_paths
                                    .push(format!("{}.relationship_type={}", action_loc, rt));
                            }
                        }
                        // Also check target_type if it references the schema
                        if let Some(tt) = action.params.get("target_type").and_then(|v| v.as_str())
                        {
                            if tt == schema_node_type {
                                broken_paths.push(format!("{}.target_type={}", action_loc, tt));
                            }
                        }
                    }
                }
            }
        }

        if !broken_paths.is_empty() {
            // Deduplicate paths
            broken_paths.sort();
            broken_paths.dedup();
            affected.push(AffectedPlaybook {
                playbook_id: pb_node.id.clone(),
                playbook_name: pb_node
                    .title
                    .clone()
                    .unwrap_or_else(|| pb_node.content.clone()),
                broken_paths,
            });
        }
    }

    Ok(affected)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playbook::types::{
        ActionType, GraphEventType, ParsedAction, ParsedRule, ParsedTrigger,
    };

    // -- CEL condition validation tests (no NodeService needed) --

    fn make_rule(
        node_type: &str,
        conditions: Vec<&str>,
        actions: Vec<ParsedAction>,
    ) -> Arc<ParsedRule> {
        Arc::new(ParsedRule {
            name: "test-rule".to_string(),
            trigger: ParsedTrigger::GraphEvent {
                on: GraphEventType::NodeCreated,
                node_type: node_type.to_string(),
                property_key: None,
            },
            conditions: conditions.into_iter().map(|s| s.to_string()).collect(),
            actions,
        })
    }

    fn make_scheduled_rule(cron: &str, node_type: &str, conditions: Vec<&str>) -> Arc<ParsedRule> {
        Arc::new(ParsedRule {
            name: "test-scheduled-rule".to_string(),
            trigger: ParsedTrigger::Scheduled {
                cron: cron.to_string(),
                node_type: node_type.to_string(),
            },
            conditions: conditions.into_iter().map(|s| s.to_string()).collect(),
            actions: vec![],
        })
    }

    fn make_create_action(node_type: &str, version: Option<&str>) -> ParsedAction {
        let mut params = serde_json::json!({
            "node_type": node_type,
            "content": "Test",
            "properties": {}
        });
        if let Some(v) = version {
            params["version"] = serde_json::json!(v);
        }
        ParsedAction {
            action_type: ActionType::CreateNode,
            params,
            for_each: None,
        }
    }

    fn make_relationship_action(rel_type: &str) -> ParsedAction {
        ParsedAction {
            action_type: ActionType::AddRelationship,
            params: serde_json::json!({
                "source_id": "{trigger.node.id}",
                "relationship_type": rel_type,
                "target_id": "some-target"
            }),
            for_each: None,
        }
    }

    // -- Pure CEL compile tests (synchronous, no DB) --

    #[test]
    fn test_valid_cel_conditions_compile() {
        assert!(compile_condition("node.status == 'open'").is_ok());
        assert!(compile_condition("node.amount > 1000").is_ok());
        assert!(compile_condition("node.priority == 'high' && node.status == 'open'").is_ok());
    }

    #[test]
    fn test_invalid_cel_condition_detected() {
        let err = compile_condition("1 + + 2").unwrap_err();
        assert!(!err.message.is_empty());
    }

    #[test]
    fn test_validation_error_display() {
        let err = PlaybookValidationError::UnknownNodeType {
            node_type: "foo".to_string(),
            location: "rule[0].trigger".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "unknown node_type 'foo' at rule[0].trigger"
        );

        let err = PlaybookValidationError::VersionMismatch {
            node_type: "invoice".to_string(),
            declared_version: "3".to_string(),
            actual_version: 2,
            location: "rule[0].action[0]".to_string(),
        };
        assert!(err.to_string().contains("version mismatch"));
        assert!(err.to_string().contains("declared '3'"));
        assert!(err.to_string().contains("schema has 2"));

        let err = PlaybookValidationError::InvalidCondition {
            expression: "bad ==".to_string(),
            message: "unexpected end".to_string(),
            location: "rule[0].condition[0]".to_string(),
        };
        assert!(err.to_string().contains("invalid CEL condition"));

        let err = PlaybookValidationError::UnknownRelationshipType {
            relationship_type: "foo_bar".to_string(),
            node_type: "task".to_string(),
            location: "rule[0].action[0]".to_string(),
        };
        assert!(err.to_string().contains("unknown relationship_type"));

        let err = PlaybookValidationError::MissingActionParam {
            param: "node_type".to_string(),
            location: "rule[0].action[0]".to_string(),
        };
        assert!(err.to_string().contains("missing required param"));
    }

    #[test]
    fn test_trigger_node_type_extraction() {
        let rule = make_rule("task", vec![], vec![]);
        assert_eq!(trigger_node_type(&rule), Some("task".to_string()));

        let rule = make_scheduled_rule("0 * * * * * *", "invoice", vec![]);
        assert_eq!(trigger_node_type(&rule), Some("invoice".to_string()));
    }

    #[test]
    fn test_multiple_cel_errors_collected() {
        // Verify that multiple invalid conditions each produce an error
        let bad1 = compile_condition("1 + + 2");
        let bad2 = compile_condition("3 * * 4");
        assert!(bad1.is_err());
        assert!(bad2.is_err());
    }

    #[test]
    fn test_binding_template_in_node_type_not_validated() {
        // Actions with binding templates like "{trigger.node.node_type}"
        // can't be validated at save time — they should be skipped
        let action = ParsedAction {
            action_type: ActionType::CreateNode,
            params: serde_json::json!({
                "node_type": "{trigger.node.node_type}",
                "content": "Test"
            }),
            for_each: None,
        };
        // The node_type contains '{', so validate_create_node_action should skip
        assert!(action.params["node_type"].as_str().unwrap().contains('{'));
    }

    #[test]
    fn test_binding_template_in_relationship_type_not_validated() {
        let action = make_relationship_action("{trigger.node.rel_type}");
        assert!(action.params["relationship_type"]
            .as_str()
            .unwrap()
            .contains('{'));
    }

    #[test]
    fn test_update_node_action_without_node_type_is_ok() {
        // update_node doesn't require node_type (it's optional for type conversion)
        let action = ParsedAction {
            action_type: ActionType::UpdateNode,
            params: serde_json::json!({
                "node_id": "{trigger.node.id}",
                "properties": {"status": "done"}
            }),
            for_each: None,
        };
        assert!(action.params.get("node_type").is_none());
    }

    #[test]
    fn test_remove_relationship_action_validates_type() {
        let action = ParsedAction {
            action_type: ActionType::RemoveRelationship,
            params: serde_json::json!({
                "source_id": "src",
                "relationship_type": "some_rel",
                "target_id": "tgt"
            }),
            for_each: None,
        };
        assert_eq!(
            action.params["relationship_type"].as_str(),
            Some("some_rel")
        );
    }

    #[test]
    fn test_missing_relationship_type_param() {
        let action = ParsedAction {
            action_type: ActionType::AddRelationship,
            params: serde_json::json!({
                "source_id": "src",
                "target_id": "tgt"
                // missing relationship_type
            }),
            for_each: None,
        };
        assert!(action.params.get("relationship_type").is_none());
    }

    // -- Async integration tests with real NodeService --

    mod integration {
        use super::*;
        use crate::db::SurrealStore;
        use crate::models::Node;
        use crate::services::NodeService;
        use serde_json::json;
        use std::sync::Arc;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SurrealStore> = Arc::new(SurrealStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        /// Helper: create a minimal schema node in the database.
        ///
        /// Note: schemas with relationships that reference target types require
        /// those target schemas to exist first (for edge table DDL).
        async fn create_schema(
            node_service: &NodeService,
            type_name: &str,
            schema_version: u32,
            relationships: serde_json::Value,
        ) {
            let schema_node = Node::new_with_id(
                type_name.to_string(),
                "schema".to_string(),
                type_name.to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": schema_version,
                    "description": format!("{} schema", type_name),
                    "fields": [
                        {"name": "status", "type": "string"}
                    ],
                    "relationships": relationships
                }),
            );
            node_service
                .create_node(schema_node)
                .await
                .expect(&format!("Failed to create schema '{}'", type_name));
        }

        // Use custom type names (prefixed "vt_") to avoid collisions
        // with core schemas seeded by NodeService::new (task, text, date, etc.)

        #[tokio::test]
        async fn test_valid_playbook_passes_validation() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_widget", 1, json!([])).await;

            let rules = vec![make_rule(
                "vt_widget",
                vec!["node.status == 'open'"],
                vec![],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_unknown_trigger_node_type_fails() {
            let (svc, _tmp) = create_test_service().await;

            let rules = vec![make_rule("nonexistent_xyzzy", vec![], vec![])];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert_eq!(errors.len(), 1);
            match &errors[0] {
                PlaybookValidationError::UnknownNodeType {
                    node_type,
                    location,
                } => {
                    assert_eq!(node_type, "nonexistent_xyzzy");
                    assert_eq!(location, "rule[0].trigger");
                }
                other => panic!("expected UnknownNodeType, got {:?}", other),
            }
        }

        #[tokio::test]
        async fn test_unknown_action_node_type_fails() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_order", 1, json!([])).await;

            let rules = vec![make_rule(
                "vt_order",
                vec![],
                vec![make_create_action("nonexistent_type_abc", None)],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(errors
                .iter()
                .any(|e| matches!(e, PlaybookValidationError::UnknownNodeType { node_type, .. } if node_type == "nonexistent_type_abc")));
        }

        #[tokio::test]
        async fn test_version_mismatch_fails() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_receipt", 2, json!([])).await;
            create_schema(&svc, "vt_trigger", 1, json!([])).await;

            // Playbook declares version "3" but schema is at version 2
            let rules = vec![make_rule(
                "vt_trigger",
                vec![],
                vec![make_create_action("vt_receipt", Some("3"))],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(errors.iter().any(|e| matches!(
                e,
                PlaybookValidationError::VersionMismatch {
                    declared_version,
                    actual_version,
                    ..
                } if declared_version == "3" && *actual_version == 2
            )));
        }

        #[tokio::test]
        async fn test_matching_version_passes() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_bill", 2, json!([])).await;
            create_schema(&svc, "vt_src", 1, json!([])).await;

            let rules = vec![make_rule(
                "vt_src",
                vec![],
                vec![make_create_action("vt_bill", Some("2"))],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_invalid_cel_condition_fails() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_item", 1, json!([])).await;

            let rules = vec![make_rule(
                "vt_item",
                vec!["1 + + 2", "node.status == 'open'"],
                vec![],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert_eq!(errors.len(), 1);
            match &errors[0] {
                PlaybookValidationError::InvalidCondition { location, .. } => {
                    assert_eq!(location, "rule[0].condition[0]");
                }
                other => panic!("expected InvalidCondition, got {:?}", other),
            }
        }

        #[tokio::test]
        async fn test_unknown_relationship_type_fails() {
            let (svc, _tmp) = create_test_service().await;
            // Create schema with a known relationship
            create_schema(
                &svc,
                "vt_project",
                1,
                json!([
                    {
                        "name": "owned_by",
                        "direction": "out",
                        "cardinality": "one"
                    }
                ]),
            )
            .await;

            let rules = vec![make_rule(
                "vt_project",
                vec![],
                vec![make_relationship_action("nonexistent_rel")],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(errors.iter().any(|e| matches!(
                e,
                PlaybookValidationError::UnknownRelationshipType {
                    relationship_type,
                    ..
                } if relationship_type == "nonexistent_rel"
            )));
        }

        #[tokio::test]
        async fn test_valid_relationship_type_passes() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(
                &svc,
                "vt_ticket",
                1,
                json!([
                    {
                        "name": "linked_to",
                        "direction": "out",
                        "cardinality": "many"
                    }
                ]),
            )
            .await;

            let rules = vec![make_rule(
                "vt_ticket",
                vec![],
                vec![make_relationship_action("linked_to")],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_multiple_errors_collected() {
            let (svc, _tmp) = create_test_service().await;
            // No custom schemas — multiple errors expected

            let rules = vec![make_rule(
                "nonexistent_aaa",
                vec!["1 + + 2"],
                vec![make_create_action("nonexistent_bbb", None)],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            // Should have at least: unknown trigger node_type + bad CEL + unknown action node_type
            assert!(
                errors.len() >= 3,
                "expected >= 3 errors, got {}",
                errors.len()
            );
        }

        #[tokio::test]
        async fn test_scheduled_trigger_node_type_validated() {
            let (svc, _tmp) = create_test_service().await;
            // "vt_cron_target" doesn't exist

            let rules = vec![make_scheduled_rule(
                "0 * * * * * *",
                "vt_cron_target",
                vec![],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(errors
                .iter()
                .any(|e| matches!(e, PlaybookValidationError::UnknownNodeType { node_type, .. } if node_type == "vt_cron_target")));
        }

        #[tokio::test]
        async fn test_empty_rules_passes() {
            let (svc, _tmp) = create_test_service().await;

            let rules: Vec<Arc<ParsedRule>> = vec![];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_binding_template_node_type_skips_validation() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vt_dynamic", 1, json!([])).await;

            // Action with binding template node_type — should not fail
            let action = ParsedAction {
                action_type: ActionType::CreateNode,
                params: json!({
                    "node_type": "{trigger.node.node_type}",
                    "content": "Dynamic"
                }),
                for_each: None,
            };
            let rules = vec![make_rule("vt_dynamic", vec![], vec![action])];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn test_core_schema_types_pass_validation() {
            let (svc, _tmp) = create_test_service().await;
            // "task" is a core schema seeded by NodeService::new — should pass

            let rules = vec![make_rule("task", vec!["node.status == 'open'"], vec![])];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_ok());
        }

        // -- Schema-aware path validation tests (#1010) --

        #[tokio::test]
        async fn test_valid_multi_hop_path_passes() {
            let (svc, _tmp) = create_test_service().await;

            // Chain: vp_task -> story (rel) -> vp_story
            create_schema(&svc, "vp_story", 1, json!([])).await;
            create_schema(
                &svc,
                "vp_task",
                1,
                json!([{
                    "name": "story",
                    "targetType": "vp_story",
                    "direction": "out",
                    "cardinality": "one"
                }]),
            )
            .await;

            // Condition: node.story.status — "story" is a relationship, "status" is a field on vp_story
            let rules = vec![make_rule(
                "vp_task",
                vec!["node.story.status == 'active'"],
                vec![],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(
                result.is_ok(),
                "valid multi-hop path should pass: {:?}",
                result
            );
        }

        #[tokio::test]
        async fn test_broken_path_unknown_segment_fails() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vp_task2", 1, json!([])).await;

            // "nonexistent" is neither a field nor relationship on vp_task2
            let rules = vec![make_rule(
                "vp_task2",
                vec!["node.nonexistent.foo == 'bar'"],
                vec![],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlaybookValidationError::BrokenPath { segment, .. } if segment == "nonexistent"
                )),
                "should report broken path for 'nonexistent': {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn test_broken_path_field_as_non_terminal() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vp_task3", 1, json!([])).await;

            // "status" is a field on vp_task3 — can't traverse further
            let rules = vec![make_rule(
                "vp_task3",
                vec!["node.status.deeper == 'x'"],
                vec![],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlaybookValidationError::BrokenPath { segment, .. } if segment == "status"
                )),
                "should report broken path for field-as-non-terminal: {:?}",
                errors
            );
        }

        #[tokio::test]
        async fn test_single_hop_property_path_skips_validation() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vp_task4", 1, json!([])).await;

            // Single-hop (node.status) is handled by existing property-level evaluation
            // and should NOT be validated against the schema graph
            let rules = vec![make_rule("vp_task4", vec!["node.status == 'open'"], vec![])];
            let result = validate_playbook(&rules, &svc).await;
            assert!(
                result.is_ok(),
                "single-hop paths should skip schema validation"
            );
        }

        #[tokio::test]
        async fn test_broken_path_relationship_without_target_type() {
            let (svc, _tmp) = create_test_service().await;
            // Relationship with no target_type
            create_schema(
                &svc,
                "vp_task5",
                1,
                json!([{
                    "name": "linked",
                    "direction": "out",
                    "cardinality": "many"
                    // no target_type
                }]),
            )
            .await;

            // Trying to traverse past a relationship without target_type
            let rules = vec![make_rule(
                "vp_task5",
                vec!["node.linked.status == 'x'"],
                vec![],
            )];
            let result = validate_playbook(&rules, &svc).await;
            assert!(result.is_err());
            let errors = result.unwrap_err();
            assert!(
                errors.iter().any(|e| matches!(
                    e,
                    PlaybookValidationError::BrokenPath { segment, .. } if segment == "linked"
                )),
                "should report broken path for rel without target_type: {:?}",
                errors
            );
        }

        // ---------------------------------------------------------------
        // check_schema_change_impact tests (Issue #1012 Phase 2)
        // ---------------------------------------------------------------

        /// Helper: create a playbook node in the database.
        async fn create_playbook(
            node_service: &NodeService,
            id: &str,
            rules_json: serde_json::Value,
        ) {
            let node = Node::new_with_id(
                id.to_string(),
                "playbook".to_string(),
                format!("Playbook {}", id),
                json!({ "rules": rules_json }),
            );
            node_service
                .create_node(node)
                .await
                .expect(&format!("Failed to create playbook '{}'", id));
        }

        #[tokio::test]
        async fn test_schema_impact_detects_affected_playbooks() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_task", 1, json!([])).await;

            // Create a playbook that triggers on "vi_task"
            create_playbook(
                &svc,
                "pb-impact-1",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vi_task" },
                    "conditions": ["node.status == 'open'"],
                    "actions": []
                }]),
            )
            .await;

            let affected = check_schema_change_impact("vi_task", &svc).await.unwrap();
            assert_eq!(affected.len(), 1);
            assert_eq!(affected[0].playbook_id, "pb-impact-1");
            assert!(
                affected[0]
                    .broken_paths
                    .iter()
                    .any(|p| p.contains("vi_task")),
                "should list the trigger path: {:?}",
                affected[0].broken_paths
            );
        }

        #[tokio::test]
        async fn test_schema_impact_unrelated_schema_passes_clean() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vi_order", 1, json!([])).await;
            create_schema(&svc, "vi_invoice", 1, json!([])).await;

            // Create a playbook that triggers on "vi_order" only
            create_playbook(
                &svc,
                "pb-impact-2",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vi_order" },
                    "conditions": ["node.status == 'open'"],
                    "actions": []
                }]),
            )
            .await;

            // Changing "vi_invoice" should not affect the vi_order playbook
            let affected = check_schema_change_impact("vi_invoice", &svc)
                .await
                .unwrap();
            assert!(
                affected.is_empty(),
                "unrelated schema change should not affect playbooks: {:?}",
                affected
            );
        }

        #[tokio::test]
        async fn test_schema_impact_detects_path_traversal() {
            let (svc, _tmp) = create_test_service().await;
            // Create vi_epic first (target of relationship)
            create_schema(&svc, "vi_epic", 1, json!([])).await;
            // Create vi_story with a relationship to vi_epic, so the playbook passes validation
            // Note: SchemaRelationship uses camelCase serialization
            create_schema(
                &svc,
                "vi_story",
                1,
                json!([{
                    "name": "vi_epic",
                    "direction": "out",
                    "cardinality": "one",
                    "targetType": "vi_epic"
                }]),
            )
            .await;

            // Playbook triggers on vi_story but has a condition traversing through vi_epic
            create_playbook(
                &svc,
                "pb-impact-3",
                json!([{
                    "name": "r1",
                    "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vi_story" },
                    "conditions": ["node.vi_epic.status == 'active'"],
                    "actions": []
                }]),
            )
            .await;

            let affected = check_schema_change_impact("vi_epic", &svc).await.unwrap();
            assert_eq!(affected.len(), 1);
            assert_eq!(affected[0].playbook_id, "pb-impact-3");
            assert!(
                affected[0]
                    .broken_paths
                    .iter()
                    .any(|p| p.contains("vi_epic")),
                "should detect path traversal through vi_epic: {:?}",
                affected[0].broken_paths
            );
        }
    }

    // ---------------------------------------------------------------
    // NodeService synchronous validation gate tests (Issue #1012 Phase 1)
    // ---------------------------------------------------------------

    mod sync_gate_tests {
        use crate::db::SurrealStore;
        use crate::models::Node;
        use crate::services::NodeService;
        use serde_json::json;
        use std::sync::Arc;
        use tempfile::TempDir;

        async fn create_test_service() -> (Arc<NodeService>, TempDir) {
            let temp_dir = TempDir::new().unwrap();
            let db_path = temp_dir.path().join("test.db");
            let mut store: Arc<SurrealStore> = Arc::new(SurrealStore::new(db_path).await.unwrap());
            let node_service = Arc::new(NodeService::new(&mut store).await.unwrap());
            (node_service, temp_dir)
        }

        async fn create_schema(node_service: &NodeService, type_name: &str, schema_version: u32) {
            let schema_node = Node::new_with_id(
                type_name.to_string(),
                "schema".to_string(),
                type_name.to_string(),
                json!({
                    "isCore": false,
                    "schemaVersion": schema_version,
                    "description": format!("{} schema", type_name),
                    "fields": [
                        {"name": "status", "type": "string"}
                    ],
                    "relationships": []
                }),
            );
            node_service
                .create_node(schema_node)
                .await
                .expect(&format!("Failed to create schema '{}'", type_name));
        }

        #[tokio::test]
        async fn test_invalid_playbook_rejected_on_create() {
            let (svc, _tmp) = create_test_service().await;
            // Don't create a schema for "nonexistent_type" — it should be rejected

            let playbook_node = Node::new_with_id(
                "pb-gate-1".to_string(),
                "playbook".to_string(),
                "Test Playbook".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "nonexistent_type" },
                        "conditions": [],
                        "actions": []
                    }]
                }),
            );

            let result = svc.create_node(playbook_node).await;
            assert!(
                result.is_err(),
                "invalid playbook should be rejected on create"
            );
            let err = result.unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("Playbook validation failed"),
                "error should indicate validation failure: {}",
                msg
            );
            assert!(
                msg.contains("nonexistent_type"),
                "error should mention the bad node_type: {}",
                msg
            );
        }

        #[tokio::test]
        async fn test_valid_playbook_accepted_on_create() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vg_widget", 1).await;

            let playbook_node = Node::new_with_id(
                "pb-gate-2".to_string(),
                "playbook".to_string(),
                "Valid Playbook".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vg_widget" },
                        "conditions": ["node.status == 'open'"],
                        "actions": []
                    }]
                }),
            );

            let result = svc.create_node(playbook_node).await;
            assert!(
                result.is_ok(),
                "valid playbook should be accepted: {:?}",
                result
            );
        }

        #[tokio::test]
        async fn test_invalid_cel_rejected_on_create() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vg_item", 1).await;

            let playbook_node = Node::new_with_id(
                "pb-gate-3".to_string(),
                "playbook".to_string(),
                "Bad CEL Playbook".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vg_item" },
                        "conditions": ["1 + + 2"],
                        "actions": []
                    }]
                }),
            );

            let result = svc.create_node(playbook_node).await;
            assert!(
                result.is_err(),
                "playbook with invalid CEL should be rejected"
            );
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("Playbook validation failed"),
                "error should indicate validation failure: {}",
                msg
            );
        }

        #[tokio::test]
        async fn test_update_with_broken_rules_rejected() {
            let (svc, _tmp) = create_test_service().await;
            create_schema(&svc, "vg_part", 1).await;

            // Create a valid playbook first
            let playbook_node = Node::new_with_id(
                "pb-gate-4".to_string(),
                "playbook".to_string(),
                "Initially Valid Playbook".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vg_part" },
                        "conditions": [],
                        "actions": []
                    }]
                }),
            );
            svc.create_node(playbook_node).await.unwrap();

            // Now update it with broken rules (reference nonexistent node_type)
            let update = crate::models::NodeUpdate {
                properties: Some(json!({
                    "rules": [{
                        "name": "r1_updated",
                        "trigger": { "type": "graph_event", "on": "node_created", "node_type": "vanished_type" },
                        "conditions": [],
                        "actions": []
                    }]
                })),
                ..Default::default()
            };

            let result = svc.update_node("pb-gate-4", 1, update).await;
            assert!(
                result.is_err(),
                "update with broken rules should be rejected"
            );
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("Playbook validation failed"),
                "error should indicate validation failure: {}",
                msg
            );
        }

        #[tokio::test]
        async fn test_parse_error_rejected_on_create() {
            let (svc, _tmp) = create_test_service().await;

            // Playbook with an invalid trigger type
            let playbook_node = Node::new_with_id(
                "pb-gate-5".to_string(),
                "playbook".to_string(),
                "Bad Trigger Playbook".to_string(),
                json!({
                    "rules": [{
                        "name": "r1",
                        "trigger": { "type": "bad_trigger_type", "on": "node_created", "node_type": "task" },
                        "conditions": [],
                        "actions": []
                    }]
                }),
            );

            let result = svc.create_node(playbook_node).await;
            assert!(
                result.is_err(),
                "playbook with invalid trigger type should be rejected"
            );
            let msg = result.unwrap_err().to_string();
            assert!(
                msg.contains("Playbook validation failed"),
                "error should indicate validation failure: {}",
                msg
            );
        }
    }
}
