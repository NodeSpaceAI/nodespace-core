//! Workspace context assembly for AI agent prompts.
//!
//! Builds a compact representation of entity types, collections, and active
//! playbooks from the database. The output is formatted as a token-efficient
//! string suitable for injection into a small-model system prompt.

use crate::services::{CollectionService, NodeService};
use std::sync::Arc;

use super::OpsError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Assembled workspace context from the database.
pub struct WorkspaceContext {
    pub entity_types: Vec<EntityTypeInfo>,
    pub collections: Vec<String>,
    pub active_playbooks: Vec<PlaybookInfo>,
}

/// Description of a single entity (node) type.
pub struct EntityTypeInfo {
    pub type_id: String,
    pub display_name: String,
    pub is_core: bool,
    pub description: String,
    pub fields: Vec<FieldInfo>,
    pub relationships: Vec<RelInfo>,
    pub title_template: Option<String>,
}

/// A field within an entity type.
pub struct FieldInfo {
    pub name: String,
    pub field_type: String,
    /// Populated for enum fields.
    pub enum_values: Option<Vec<String>>,
}

/// A relationship from an entity type to another type.
pub struct RelInfo {
    pub name: String,
    pub target_type: String,
}

/// An active playbook.
pub struct PlaybookInfo {
    pub name: String,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Build workspace context by querying schemas, collections, and playbooks.
pub async fn build_workspace_context(
    node_service: &Arc<NodeService>,
) -> Result<WorkspaceContext, OpsError> {
    // Fetch schemas (empty vec on error — fresh database is fine)
    let schemas = node_service.get_all_schemas().await.unwrap_or_default();

    // Fetch collection names
    let collection_service = CollectionService::new(node_service.store(), node_service);
    let collections = collection_service
        .get_all_collection_names()
        .await
        .unwrap_or_default();

    // Fetch active playbooks
    let playbook_nodes = node_service
        .query_nodes_by_type("playbook", Some("active"))
        .await
        .unwrap_or_default();

    // Convert schemas to EntityTypeInfo
    let entity_types: Vec<EntityTypeInfo> = schemas
        .into_iter()
        .map(|schema| {
            let fields: Vec<FieldInfo> = schema
                .fields
                .iter()
                .map(|f| {
                    let enum_values = if f.field_type == "enum" {
                        let mut vals = Vec::new();
                        if let Some(core_vals) = &f.core_values {
                            vals.extend(core_vals.iter().map(|v| v.label.clone()));
                        }
                        if let Some(user_vals) = &f.user_values {
                            vals.extend(user_vals.iter().map(|v| v.label.clone()));
                        }
                        if vals.is_empty() {
                            None
                        } else {
                            Some(vals)
                        }
                    } else {
                        None
                    };
                    FieldInfo {
                        name: f.name.clone(),
                        field_type: f.field_type.clone(),
                        enum_values,
                    }
                })
                .collect();

            let relationships: Vec<RelInfo> = schema
                .relationships
                .iter()
                .map(|r| RelInfo {
                    name: r.name.clone(),
                    target_type: r
                        .target_type
                        .clone()
                        .unwrap_or_else(|| "any".to_string()),
                })
                .collect();

            EntityTypeInfo {
                type_id: schema.id.clone(),
                display_name: schema.content.clone(),
                is_core: schema.is_core,
                description: schema.description.clone(),
                fields,
                relationships,
                title_template: schema.title_template.clone(),
            }
        })
        .collect();

    // Convert playbook nodes
    let active_playbooks: Vec<PlaybookInfo> = playbook_nodes
        .into_iter()
        .map(|node| PlaybookInfo {
            name: node.content.clone(),
            description: node
                .properties
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect();

    Ok(WorkspaceContext {
        entity_types,
        collections,
        active_playbooks,
    })
}

// ---------------------------------------------------------------------------
// Formatter
// ---------------------------------------------------------------------------

impl WorkspaceContext {
    /// Format context as a compact string for injection into a system prompt.
    ///
    /// `max_chars` is a rough character budget. If the formatted string exceeds
    /// it, custom schemas are prioritized over core types and the output is
    /// truncated with an ellipsis note.
    pub fn format_for_prompt(&self, max_chars: usize) -> String {
        let mut out = String::new();

        // Entity types section
        if !self.entity_types.is_empty() {
            out.push_str("ENTITY TYPES:\n");

            // Sort: custom types first (more important for the user), then core
            let mut sorted: Vec<&EntityTypeInfo> = self.entity_types.iter().collect();
            sorted.sort_by_key(|e| e.is_core); // false < true, so custom first

            for et in &sorted {
                let core_tag = if et.is_core { " (core)" } else { "" };
                let mut line = format!("- {}: {}{}", et.type_id, et.display_name, core_tag);

                // Fields
                if !et.fields.is_empty() {
                    let fields_str: Vec<String> = et
                        .fields
                        .iter()
                        .map(|f| {
                            if let Some(vals) = &f.enum_values {
                                format!("{}(enum: {})", f.name, vals.join("/"))
                            } else {
                                format!("{}({})", f.name, f.field_type)
                            }
                        })
                        .collect();
                    line.push_str(&format!(" -- fields: {}", fields_str.join(", ")));
                }

                // Relationships
                if !et.relationships.is_empty() {
                    let rels_str: Vec<String> = et
                        .relationships
                        .iter()
                        .map(|r| format!("{} {}", r.name, r.target_type))
                        .collect();
                    line.push_str(&format!(" -- rels: {}", rels_str.join(", ")));
                }

                // Title template
                if let Some(tmpl) = &et.title_template {
                    line.push_str(&format!(" -- title: \"{}\"", tmpl));
                }

                line.push('\n');

                // Budget check: if adding this line would exceed the budget,
                // stop and add a truncation note.
                if out.len() + line.len() > max_chars.saturating_sub(60) {
                    out.push_str("  (... more types available via get_all_schemas)\n");
                    break;
                }
                out.push_str(&line);
            }
        }

        // Collections section
        if !self.collections.is_empty() {
            let section = format!("\nCOLLECTIONS: {}\n", self.collections.join(", "));
            if out.len() + section.len() <= max_chars {
                out.push_str(&section);
            }
        }

        // Playbooks section
        if !self.active_playbooks.is_empty() {
            let header = "\nACTIVE PLAYBOOKS:\n";
            if out.len() + header.len() < max_chars {
                out.push_str(header);
                for pb in &self.active_playbooks {
                    let line = if pb.description.is_empty() {
                        format!("- \"{}\"\n", pb.name)
                    } else {
                        format!("- \"{}\": {}\n", pb.name, pb.description)
                    };
                    if out.len() + line.len() > max_chars {
                        break;
                    }
                    out.push_str(&line);
                }
            }
        }

        out
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_context() -> WorkspaceContext {
        WorkspaceContext {
            entity_types: vec![
                EntityTypeInfo {
                    type_id: "customer".into(),
                    display_name: "Customer".into(),
                    is_core: false,
                    description: "Customer entity".into(),
                    fields: vec![
                        FieldInfo {
                            name: "company".into(),
                            field_type: "text".into(),
                            enum_values: None,
                        },
                        FieldInfo {
                            name: "status".into(),
                            field_type: "enum".into(),
                            enum_values: Some(vec!["Active".into(), "Churned".into()]),
                        },
                        FieldInfo {
                            name: "email".into(),
                            field_type: "text".into(),
                            enum_values: None,
                        },
                    ],
                    relationships: vec![RelInfo {
                        name: "has".into(),
                        target_type: "invoice".into(),
                    }],
                    title_template: Some("{company}".into()),
                },
                EntityTypeInfo {
                    type_id: "task".into(),
                    display_name: "Task".into(),
                    is_core: true,
                    description: "Task tracking".into(),
                    fields: vec![
                        FieldInfo {
                            name: "status".into(),
                            field_type: "enum".into(),
                            enum_values: Some(vec![
                                "Open".into(),
                                "In Progress".into(),
                                "Done".into(),
                            ]),
                        },
                        FieldInfo {
                            name: "priority".into(),
                            field_type: "enum".into(),
                            enum_values: Some(vec![
                                "Low".into(),
                                "Medium".into(),
                                "High".into(),
                            ]),
                        },
                    ],
                    relationships: vec![],
                    title_template: None,
                },
            ],
            collections: vec!["Projects".into(), "Clients".into(), "Research".into()],
            active_playbooks: vec![PlaybookInfo {
                name: "Task completion".into(),
                description: "When task.status -> Done, evaluate project progress".into(),
            }],
        }
    }

    #[test]
    fn format_for_prompt_includes_all_sections() {
        let ctx = sample_context();
        let output = ctx.format_for_prompt(4000);

        assert!(output.contains("ENTITY TYPES:"));
        assert!(output.contains("customer: Customer"));
        assert!(output.contains("task: Task (core)"));
        assert!(output.contains("COLLECTIONS:"));
        assert!(output.contains("Projects"));
        assert!(output.contains("ACTIVE PLAYBOOKS:"));
        assert!(output.contains("Task completion"));
    }

    #[test]
    fn format_for_prompt_shows_enum_values() {
        let ctx = sample_context();
        let output = ctx.format_for_prompt(4000);

        assert!(output.contains("status(enum: Active/Churned)"));
        assert!(output.contains("company(text)"));
    }

    #[test]
    fn format_for_prompt_shows_relationships() {
        let ctx = sample_context();
        let output = ctx.format_for_prompt(4000);

        assert!(output.contains("rels: has invoice"));
    }

    #[test]
    fn format_for_prompt_shows_title_template() {
        let ctx = sample_context();
        let output = ctx.format_for_prompt(4000);

        assert!(output.contains("title: \"{company}\""));
    }

    #[test]
    fn format_for_prompt_custom_types_before_core() {
        let ctx = sample_context();
        let output = ctx.format_for_prompt(4000);

        let customer_pos = output.find("customer:").unwrap();
        let task_pos = output.find("task:").unwrap();
        assert!(
            customer_pos < task_pos,
            "Custom types should appear before core types"
        );
    }

    #[test]
    fn format_for_prompt_truncates_on_budget() {
        let ctx = sample_context();
        // Very small budget — should truncate
        let output = ctx.format_for_prompt(100);
        assert!(output.len() <= 200); // some slack for the truncation message
    }

    #[test]
    fn format_for_prompt_empty_context() {
        let ctx = WorkspaceContext {
            entity_types: vec![],
            collections: vec![],
            active_playbooks: vec![],
        };
        let output = ctx.format_for_prompt(4000);
        assert!(output.is_empty());
    }
}
