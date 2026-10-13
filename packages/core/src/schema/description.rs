//! A type in plain language (ADR-100 §5).
//!
//! A schema is a definition, and read raw it does not say what a field means,
//! what may be added to it or what it extends. [`describe_schema`] renders it,
//! from stored data only and with no model call: whether the type is built in
//! or the user's own and what that lets a person change, what it extends, then
//! each field with its type, allowed values, whether it is required, its
//! default and its description. It is what a chat bound to a type opens on.
//!
//! The wording stays on the stored names. A field is called what it is called
//! in a saved query, a play and a title template, so a person reading this and
//! the agent reading the definition are talking about the same things.

use crate::models::schema::{RelationshipCardinality, RelationshipDirection, SchemaField};
use crate::models::{SchemaNode, SchemaProtectionLevel};
use nodespace_types::SchemaFieldType;
use std::collections::{HashMap, HashSet};

/// What a description is rendered from.
pub struct SchemaDescription<'a> {
    pub schema: &'a SchemaNode,
    /// What the type is for, from the schema node's description subtree.
    pub purpose: Option<&'a str>,
    /// The effective fields: the type's own, then the inherited ones.
    pub fields: &'a [SchemaField],
    /// The schema that declares each field, by field name.
    pub owners: &'a HashMap<String, String>,
    /// The ids of the built-in schemas among those that declare a field. A
    /// field one of them declares is fixed, whatever its own protection says:
    /// a built-in type is closed (ADR-086).
    pub built_in: &'a HashSet<String>,
}

/// The type in a paragraph and a list of its fields. `link` is the markdown
/// link to the type, which the caller makes.
pub fn describe_schema(link: &str, input: &SchemaDescription<'_>) -> String {
    let SchemaDescription {
        schema,
        purpose,
        fields,
        owners,
        built_in,
    } = input;
    let mut message = format!("Here is what {link} is.");
    if let Some(purpose) = purpose.map(one_line).filter(|p| !p.is_empty()) {
        message.push(' ');
        message.push_str(&purpose);
    }

    message.push_str("\n\n");
    message.push_str(&standing(schema));
    if schema.is_abstract {
        message.push_str(
            " It is abstract: no node is created as it directly, only as a type that extends it.",
        );
    }

    message.push_str("\n\n");
    message.push_str(&match fields.len() {
        0 => "It has no fields.".to_string(),
        1 => "It has one field.".to_string(),
        count => format!("It has {count} fields."),
    });
    for field in *fields {
        message.push('\n');
        let owner = owners.get(&field.name);
        let fixed = field.protection != SchemaProtectionLevel::User
            || owner.is_some_and(|owner| built_in.contains(owner));
        message.push_str(&field_line(field, owner, &schema.envelope.id, fixed));
    }

    if !schema.relationships.is_empty() {
        message.push_str("\n\nIt declares these relationships:");
        for relationship in &schema.relationships {
            let reaches = relationship.target_type.as_deref().unwrap_or("any type");
            let direction = match relationship.direction {
                RelationshipDirection::Out => "to",
                RelationshipDirection::In => "from",
            };
            let cardinality = match relationship.cardinality {
                RelationshipCardinality::One => "one",
                RelationshipCardinality::Many => "many",
            };
            message.push_str(&format!(
                "\n- **{}**: {direction} {reaches} ({cardinality})",
                relationship.name
            ));
            if let Some(description) = relationship.description.as_deref().map(one_line) {
                if !description.is_empty() {
                    message.push_str(&format!(". {description}"));
                }
            }
        }
    }

    if let Some(template) = schema.title_template.as_deref() {
        message.push_str(&format!("\n\nIts title is built from `{template}`."));
    }
    message
}

/// Whether the type is built in or the user's own, what extends what, and what
/// that lets a person change.
fn standing(schema: &SchemaNode) -> String {
    let extends = schema
        .extends
        .as_deref()
        .map(|parent| format!(" It extends `{parent}`, so it has that type's fields as well."))
        .unwrap_or_default();
    if schema.is_core {
        format!(
            "It is a built-in type, so the fields it declares are fixed and cannot be changed or \
             removed. You can add a field of your own under a prefix such as `custom:`, and add \
             your own values to a list that accepts them.{extends}"
        )
    } else {
        format!("It is your own type, so its fields can be added, changed and removed.{extends}")
    }
}

/// One field: its stored name, type and description, then what constrains it.
fn field_line(field: &SchemaField, owner: Option<&String>, schema_id: &str, fixed: bool) -> String {
    let mut kind = field.field_type.as_str().to_string();
    if field.field_type == SchemaFieldType::Array {
        if let Some(item) = field.item_type {
            kind = format!("array of {}", item.as_str());
        }
    }
    let mut line = format!("- **{}** ({kind})", field.name);
    if let Some(description) = field.description.as_deref().map(one_line) {
        if !description.is_empty() {
            line.push_str(&format!(": {description}"));
        }
    }

    let mut notes: Vec<String> = Vec::new();
    let values: Vec<&str> = field
        .core_values
        .iter()
        .flatten()
        .chain(field.user_values.iter().flatten())
        .map(|v| v.value.as_str())
        .collect();
    if !values.is_empty() {
        notes.push(format!("Values: {}.", values.join(", ")));
        if field.extensible == Some(true) {
            notes.push("More values can be added.".to_string());
        }
    }
    if field.required == Some(true) {
        notes.push("Required.".to_string());
    }
    if let Some(default) = &field.default {
        notes.push(format!("Default: {}.", plain(default)));
    }
    if fixed {
        notes.push("Built in: it cannot be changed or removed.".to_string());
    }
    if let Some(owner) = owner.filter(|owner| owner.as_str() != schema_id) {
        notes.push(format!("From `{owner}`."));
    }
    if !notes.is_empty() {
        let sep = if line.ends_with(')') { ": " } else { " " };
        line.push_str(sep);
        line.push_str(&notes.join(" "));
    }
    line
}

/// A stored value as text: a string without its quotes.
fn plain(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Author text on one line, so it cannot break the list it is shown in.
fn one_line(text: impl AsRef<str>) -> String {
    text.as_ref()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_types::EnumValue;

    fn enum_field(name: &str, values: &[&str]) -> SchemaField {
        SchemaField {
            name: name.to_string(),
            field_type: SchemaFieldType::Enum,
            core_values: Some(
                values
                    .iter()
                    .map(|v| EnumValue {
                        value: (*v).to_string(),
                        label: (*v).to_string(),
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }

    fn describe(schema: &SchemaNode, fields: &[SchemaField]) -> String {
        let owners: HashMap<String, String> = fields
            .iter()
            .map(|f| (f.name.clone(), schema.envelope.id.clone()))
            .collect();
        let built_in: HashSet<String> = schema
            .is_core
            .then(|| schema.envelope.id.clone())
            .into_iter()
            .collect();
        describe_schema(
            "[Ticket](nodespace://ticket)",
            &SchemaDescription {
                schema,
                purpose: Some("A support request.\nOne per customer issue."),
                fields,
                owners: &owners,
                built_in: &built_in,
            },
        )
    }

    #[test]
    fn a_users_own_type_is_open() {
        let mut schema = SchemaNode::new("ticket", "Ticket");
        schema.title_template = Some("{summary}".to_string());
        let fields = [
            SchemaField {
                name: "summary".to_string(),
                description: Some("What went wrong.".to_string()),
                required: Some(true),
                ..Default::default()
            },
            SchemaField {
                default: Some(serde_json::json!("open")),
                ..enum_field("state", &["open", "closed"])
            },
        ];

        assert_eq!(
            describe(&schema, &fields),
            "Here is what [Ticket](nodespace://ticket) is. A support request. One per customer issue.\n\
             \n\
             It is your own type, so its fields can be added, changed and removed.\n\
             \n\
             It has 2 fields.\n\
             - **summary** (text): What went wrong. Required.\n\
             - **state** (enum): Values: open, closed. Default: open.\n\
             \n\
             Its title is built from `{summary}`."
        );
    }

    #[test]
    fn a_built_in_type_says_what_cannot_change() {
        let mut schema = SchemaNode::new("task", "Task");
        schema.is_core = true;
        let status = SchemaField {
            protection: SchemaProtectionLevel::Core,
            extensible: Some(true),
            ..enum_field("status", &["open", "done"])
        };

        let text = describe(&schema, &[status]);
        assert!(
            text.contains("built-in type, so the fields it declares are fixed"),
            "{text}"
        );
        assert!(text.contains("under a prefix such as `custom:`"), "{text}");
        assert!(
            text.contains(
                "- **status** (enum): Values: open, done. More values can be added. \
                 Built in: it cannot be changed or removed."
            ),
            "{text}"
        );
    }

    #[test]
    fn an_inherited_field_names_its_owner_and_the_parent() {
        let mut schema = SchemaNode::new("bug", "Bug");
        schema.extends = Some("task".to_string());
        let inherited = SchemaField {
            name: "status".to_string(),
            ..Default::default()
        };
        let owners = HashMap::from([("status".to_string(), "task".to_string())]);
        let built_in = HashSet::from(["task".to_string()]);

        let text = describe_schema(
            "[Bug](nodespace://bug)",
            &SchemaDescription {
                schema: &schema,
                purpose: None,
                fields: &[inherited],
                owners: &owners,
                built_in: &built_in,
            },
        );
        assert!(
            text.contains("It extends `task`, so it has that type's fields as well."),
            "{text}"
        );
        // Its owner is built in, so the field is fixed here too.
        assert!(
            text.contains(
                "- **status** (text): Built in: it cannot be changed or removed. From `task`."
            ),
            "{text}"
        );
    }
}
