//! Output formatters for CLI subcommands.
//!
//! Human-readable mode emits a stable, label-prefixed layout intended for
//! interactive use. JSON mode emits the proto-as-JSON representation so the
//! output is unambiguous and scriptable.

use anyhow::{Context, Result};
use nodespace_daemon::nodespace::{
    ConflictRecord as ConflictRecordProto, ContextNode, DeleteNodeResponse, GetNodeContextResponse,
    MergeNodesResponse, NodeListResponse, PathNodes, RunSavedQueryResponse,
};

use crate::commands::skill::{
    announce_attached_skills, attached_skills_json, matched_queries_json, sanitize_for_terminal,
    write_attached_skills,
};
use nodespace_daemon::NodeData;
use nodespace_types::{CreateSchemaOutput, SchemaNode, SchemaUpdateOutput};
use serde_json::{json, Value};

pub fn print_node(node: &NodeData, json: bool) -> Result<()> {
    if json {
        let value = node_to_json(node);
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        write_human_node(node);
    }
    Ok(())
}

/// A node's lifecycle as a listing prints it: the stored value, verbatim. For
/// a command that prints its own one-line-per-node listing instead of
/// [`print_node_list`]'s blocks.
pub fn lifecycle_label(node: &NodeData) -> &str {
    &node.lifecycle_status
}

pub fn print_delete(response: &DeleteNodeResponse, json: bool) -> Result<()> {
    if json {
        let value = json!({
            "node_id": response.node_id,
            "existed": response.existed,
            "deleted_count": response.deleted_count,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else if !response.existed {
        println!("Node {} did not exist (no-op)", response.node_id);
    } else if response.title.is_empty() {
        println!("Deleted node {}", response.node_id);
    } else {
        println!(
            "Deleted \"{}\" ({}){}",
            response.title,
            response.node_id,
            nested_clause(response.deleted_count.saturating_sub(1))
        );
    }
    Ok(())
}

/// The first step of a node delete: what it would remove, and the one
/// command that removes exactly that.
///
/// `routing` is the global `--socket`/`--database` flags the preview ran
/// with, repeated so the printed command reaches the same database.
pub fn print_delete_preview(
    response: &DeleteNodeResponse,
    routing: &[String],
    json: bool,
) -> Result<()> {
    if !response.existed {
        return print_delete(response, json);
    }
    let confirm_command = delete_confirm_command(response, routing);
    if json {
        let value = delete_preview_json(response, &confirm_command);
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "Would delete \"{}\" ({}, {} v{}){}.",
            response.title,
            response.node_id,
            response.node_type,
            response.version,
            nested_clause(response.descendant_count)
        );
        println!("Nothing deleted. To delete exactly this:");
        println!("  {confirm_command}");
    }
    Ok(())
}

/// The command that deletes exactly the state a preview showed.
pub fn delete_confirm_command(response: &DeleteNodeResponse, routing: &[String]) -> String {
    let mut words = vec!["nodespace".to_string()];
    words.extend(routing.iter().map(|w| shell_quote(w)));
    words.push(format!(
        "node delete {} --version {} --descendants {}",
        shell_quote(&response.node_id),
        response.version,
        response.descendant_count
    ));
    words.join(" ")
}

/// The `--json` shape of a delete preview — what an agent parses.
pub fn delete_preview_json(response: &DeleteNodeResponse, confirm_command: &str) -> Value {
    json!({
        "node_id": response.node_id,
        "existed": true,
        "deleted": false,
        "title": response.title,
        "node_type": response.node_type,
        "version": response.version,
        "descendant_count": response.descendant_count,
        "confirm_command": confirm_command,
    })
}

/// Quote a word for a POSIX shell line, leaving plain words bare.
fn shell_quote(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

fn nested_clause(count: u64) -> String {
    match count {
        0 => String::new(),
        1 => " and 1 nested node".to_string(),
        n => format!(" and {n} nested nodes"),
    }
}

pub fn print_node_list(response: &NodeListResponse, json: bool) -> Result<()> {
    if json {
        let value = json!({
            "count": response.count,
            "collection_id": response.collection_id,
            "nodes": response.nodes.iter().map(node_to_json).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if response.nodes.is_empty() {
        println!("No nodes returned (count: 0)");
        return Ok(());
    }

    println!("{} node(s):", response.count);
    for (idx, node) in response.nodes.iter().enumerate() {
        if idx > 0 {
            println!();
        }
        write_human_node(node);
    }
    Ok(())
}

/// A saved query's result: its nodes, as [`print_node_list`] prints them, and
/// the skills attached to the query node, each inside the banner a skill
/// fetch prints.
pub fn print_saved_query_run(response: &RunSavedQueryResponse, json: bool) -> Result<()> {
    if json {
        let mut value = json!({
            "count": response.count,
            "nodes": response.nodes.iter().map(node_to_json).collect::<Vec<_>>(),
            "attached_skills": attached_skills_json(&response.skills, &response.schemas),
        });
        // Set only when the run returned as many results as its limit allows.
        if response.limit_reached {
            value["limit_reached"] = json!(true);
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    let out = &mut std::io::stdout();
    let tag = announce_attached_skills(out, &response.skills, &response.schemas)?;
    print_node_list(
        &NodeListResponse {
            nodes: response.nodes.clone(),
            count: response.count,
            collection_id: String::new(),
        },
        false,
    )?;
    if response.limit_reached {
        println!(
            "\nThe run returned as many results as its limit allows; the query may match more."
        );
    }
    match tag {
        Some(tag) => write_attached_skills(out, &response.skills, &response.schemas, &tag),
        None => Ok(()),
    }
}

/// A saved query's result with each item's context: the item, what its
/// type's context paths reach, the skills that apply to it and the version
/// of that read. Each skill is printed once, after the items, inside the
/// banner a skill fetch prints; an item names its skills by id.
pub fn print_saved_query_context_run(response: &RunSavedQueryResponse, json: bool) -> Result<()> {
    if json {
        let items: Vec<Value> = response
            .items
            .iter()
            .map(|item| {
                let skills: Vec<Value> = item
                    .skills
                    .iter()
                    .map(|skill| {
                        let mut entry = json!({
                            "id": skill.skill_id,
                            "attached_to": skill.attached_to,
                        });
                        if !skill.matched_queries.is_empty() {
                            entry["matched_queries"] = matched_queries_json(&skill.matched_queries);
                        }
                        entry
                    })
                    .collect();
                json!({
                    "node": item.node.as_ref().map(context_node_to_json),
                    "paths": path_groups_json(&item.paths),
                    "skills": skills,
                    "version": item.version,
                })
            })
            .collect();
        let mut value = json!({
            "count": response.count,
            "items": items,
            "attached_skills": attached_skills_json(&response.skills, &response.schemas),
        });
        if response.limit_reached {
            value["limit_reached"] = json!(true);
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    let out = &mut std::io::stdout();
    let tag = announce_attached_skills(out, &response.skills, &response.schemas)?;
    if response.items.is_empty() {
        println!("No nodes returned (count: 0)");
    } else {
        println!("{} item(s):", response.count);
    }
    for (idx, item) in response.items.iter().enumerate() {
        println!();
        println!(
            "--- item {} (context version {}) ---",
            idx + 1,
            item.version
        );
        if let Some(node) = &item.node {
            write_human_context_node(node);
        }
        write_human_paths(&item.paths);
        if !item.skills.is_empty() {
            println!();
            println!("skills (each printed once, below):");
            for skill in &item.skills {
                let mut through: Vec<String> = skill
                    .attached_to
                    .iter()
                    .map(|id| format!("attached to {}", sanitize_for_terminal(id)))
                    .collect();
                through.extend(skill.matched_queries.iter().map(|query| {
                    format!(
                        "matches \"{}\" ({})",
                        sanitize_for_terminal(&query.title),
                        sanitize_for_terminal(&query.id)
                    )
                }));
                println!(
                    "    skill/{}  {}",
                    sanitize_for_terminal(&skill.skill_id),
                    through.join("; ")
                );
            }
        }
    }
    if response.limit_reached {
        println!("\nThe run returned as many items as its limit allows; the query may match more.");
    }
    match tag {
        Some(tag) => write_attached_skills(out, &response.skills, &response.schemas, &tag),
        None => Ok(()),
    }
}

/// What each path of a context read reached, in the CLI's JSON shape.
fn path_groups_json(paths: &[PathNodes]) -> Vec<Value> {
    paths
        .iter()
        .map(|reached| {
            let mut group = json!({
                "path": reached.path,
                "count": reached.nodes.len(),
                "nodes": reached.nodes.iter().map(context_node_to_json).collect::<Vec<_>>(),
            });
            // Set only when the path reached more nodes than one read
            // returns.
            if reached.limit_reached {
                group["limit_reached"] = json!(true);
            }
            group
        })
        .collect()
}

fn write_human_paths(paths: &[PathNodes]) {
    for reached in paths {
        println!();
        println!(
            "path {} ({} node(s){}):",
            reached.path,
            reached.nodes.len(),
            if reached.limit_reached {
                "; the path reaches more, these are the first"
            } else {
                ""
            }
        );
        for (idx, node) in reached.nodes.iter().enumerate() {
            if idx > 0 {
                println!();
            }
            write_human_context_node(node);
        }
    }
}

/// A node as a context read returns it, in the CLI's JSON shape: the node's
/// own keys, and `checkboxes` for its direct checkbox children.
fn context_node_to_json(node: &ContextNode) -> Value {
    let mut value = node.node.as_ref().map(node_to_json).unwrap_or(Value::Null);
    if let Value::Object(map) = &mut value {
        map.insert(
            "checkboxes".to_string(),
            node.checkboxes.iter().map(node_to_json).collect(),
        );
    }
    value
}

fn write_human_context_node(node: &ContextNode) {
    if let Some(data) = &node.node {
        write_human_node(data);
    }
    if !node.checkboxes.is_empty() {
        println!("checkboxes:");
        // Graph text, like a skill's: control characters are stripped, and a
        // continuation line stays indented under its item.
        for checkbox in &node.checkboxes {
            let content = sanitize_for_terminal(&checkbox.content);
            let mut lines = content.lines();
            println!(
                "    {}  ({})",
                lines.next().unwrap_or_default(),
                sanitize_for_terminal(&checkbox.id)
            );
            for line in lines {
                println!("      {line}");
            }
        }
    }
}

/// A node read with the nodes its paths reach, grouped by path, and the
/// skills attached to any of them, each inside the banner a skill fetch
/// prints.
pub fn print_node_context(response: &GetNodeContextResponse, json: bool) -> Result<()> {
    let node = response.node.as_ref().context("daemon returned no node")?;
    if json {
        let value = json!({
            "node": context_node_to_json(node),
            "paths": path_groups_json(&response.paths),
            "attached_skills": attached_skills_json(&response.skills, &response.schemas),
            "version": response.version,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    let out = &mut std::io::stdout();
    let tag = announce_attached_skills(out, &response.skills, &response.schemas)?;
    // The daemon computed it, so it is printed ahead of anything read from
    // the graph.
    println!("context version: {}\n", response.version);
    write_human_context_node(node);
    write_human_paths(&response.paths);
    match tag {
        Some(tag) => write_attached_skills(out, &response.skills, &response.schemas, &tag),
        None => Ok(()),
    }
}

/// The version of a context read, alone: what a client compares with the
/// one it holds to learn whether anything the read returns has changed.
pub fn print_node_context_version(response: &GetNodeContextResponse, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "version": response.version }))?
        );
    } else {
        println!("{}", response.version);
    }
    Ok(())
}

/// One node in human mode. Everything read from the graph is passed through
/// [`sanitize_for_terminal`]: a node's text can carry terminal control
/// sequences, and some commands print it beside a provenance tag line that
/// such a sequence could redraw. JSON mode is left lossless.
fn write_human_node(node: &NodeData) {
    let clean = sanitize_for_terminal;
    println!("id:              {}", clean(&node.id));
    println!("type:            {}", clean(&node.node_type));
    // Absent (this node type never gets one, e.g. `date`/`schema`) is
    // distinct from present-but-empty (a title_template whose fields are all
    // still blank) — only the former omits the line; the latter still prints,
    // just with nothing after the label.
    if let Some(title) = &node.title {
        println!("title:           {}", clean(title));
    }
    println!("version:         {}", node.version);
    println!("lifecycle:       {}", node.lifecycle_status);
    println!("created_at:      {}", node.created_at);
    println!("modified_at:     {}", node.modified_at);
    // Flattened for the same reason as JSON mode: the schema-id nesting is a
    // storage detail and must not surface anywhere on the CLI.
    let properties = properties_to_json(node);
    // Object is the flattened shape; String only the malformed-JSON fallback.
    // Any other variant is unreachable, but print rather than silently drop it.
    let has_properties = match &properties {
        serde_json::Value::Object(map) => !map.is_empty(),
        serde_json::Value::String(s) => !s.is_empty() && s != "{}",
        _ => true,
    };
    if has_properties {
        println!("properties:      {}", clean(&properties.to_string()));
    }
    println!("content:");
    for line in clean(&node.content).lines() {
        println!("    {}", line);
    }
    if node.content.is_empty() {
        println!("    (empty)");
    }
    if !node.markdown.is_empty() {
        println!("markdown:");
        for line in clean(&node.markdown).lines() {
            println!("    {}", line);
        }
    }
}

/// A schema in the CLI's shape: the typed `SchemaNode` with its top-level keys
/// in snake_case, like every other CLI payload.
///
/// The keys are the ones `schema create` and `schema update` take
/// (`fields`, `relationships`, `extends`, `abstract`, `children`, `parent`,
/// `title_template`, `properties_header_summary_template`), so what a caller
/// reads here is what it writes there. Field and relationship entries keep
/// their camelCase metadata keys on both sides.
pub fn schema_to_json(schema: &SchemaNode) -> Value {
    // Destructured in full, so a field added to `SchemaNode` fails to compile
    // here until the CLI says how it prints.
    let SchemaNode {
        envelope,
        is_core,
        is_abstract,
        extends,
        children,
        parent,
        schema_version,
        fields,
        relationships,
        title_template,
        properties_header_summary_template,
        context_paths,
    } = schema;

    let mut value = json!({
        "id": envelope.id,
        "node_type": envelope.node_type,
        "content": envelope.content,
        "version": envelope.version,
        "lifecycle_status": envelope.lifecycle_status,
        "created_at": envelope.created_at,
        "modified_at": envelope.modified_at,
        "is_core": is_core,
        "schema_version": schema_version,
        "fields": fields,
        "relationships": relationships,
    });
    // Omitted when they declare nothing, as on the wire.
    if *is_abstract {
        value["abstract"] = json!(true);
    }
    if let Some(parent_type) = extends {
        value["extends"] = json!(parent_type);
    }
    if !children.is_any() {
        value["children"] = json!(children);
    }
    if !parent.is_any() {
        value["parent"] = json!(parent);
    }
    if let Some(template) = title_template {
        value["title_template"] = json!(template);
    }
    if let Some(template) = properties_header_summary_template {
        value["properties_header_summary_template"] = json!(template);
    }
    // In the dotted form `add_context_paths` and `node context --path` take.
    if !context_paths.is_empty() {
        value["context_paths"] = context_paths.iter().map(ToString::to_string).collect();
    }
    value
}

/// The JSON `schema create` prints: the created schema under the keys
/// `schema get` reads one with, plus the `description` written and any
/// `warnings`.
pub fn schema_created_to_json(created: &CreateSchemaOutput) -> Value {
    // Destructured in full, so a field added to `CreateSchemaOutput` fails to
    // compile here until the CLI says how it prints.
    let CreateSchemaOutput {
        schema_id,
        is_core,
        version,
        description,
        fields,
        extends,
        relationships,
        warnings,
    } = created;

    let mut value = json!({
        "id": schema_id,
        "is_core": is_core,
        "schema_version": version,
        "description": description,
        "fields": fields,
        "relationships": relationships,
    });
    if let Some(parent_type) = extends {
        value["extends"] = json!(parent_type);
    }
    if let Some(warnings) = warnings {
        value["warnings"] = json!(warnings);
    }
    value
}

pub fn print_schema_created(result_json: &str, json: bool) -> Result<()> {
    println!("{}", render_schema_created(result_json, json)?);
    Ok(())
}

/// What `schema create` prints: the JSON object with `--json`, a short
/// summary without.
fn render_schema_created(result_json: &str, json: bool) -> Result<String> {
    let created: CreateSchemaOutput =
        serde_json::from_str(result_json).context("daemon returned a malformed create result")?;
    if json {
        return Ok(serde_json::to_string_pretty(&schema_created_to_json(
            &created,
        ))?);
    }

    let mut lines = vec![format!("Created schema {}", created.schema_id)];
    if let Some(parent_type) = &created.extends {
        lines.push(format!("extends:         {parent_type}"));
    }
    lines.push("fields:".to_string());
    if created.fields.is_empty() {
        lines.push("    (none)".to_string());
    }
    for field in &created.fields {
        lines.push(format!("    {}: {}", field.name, field.field_type));
    }
    if !created.relationships.is_empty() {
        lines.push("relationships:".to_string());
        for relationship in &created.relationships {
            lines.push(format!(
                "    {} -> {} (reverse: {})",
                relationship.name,
                relationship.target_type.as_deref().unwrap_or("*"),
                relationship.reverse_name
            ));
        }
    }
    for warning in created.warnings.iter().flatten() {
        lines.push(format!("warning: {warning}"));
    }
    Ok(lines.join("\n"))
}

/// The JSON `schema update` prints: what the update changed, in snake_case.
/// A count is present only for a kind of change the update made.
pub fn schema_updated_to_json(updated: &SchemaUpdateOutput) -> Value {
    let mut value = json!({ "id": updated.schema_id, "success": updated.success });
    for (key, count) in schema_update_counts(updated) {
        value[key] = json!(count);
    }
    if let Some(plays) = &updated.affected_plays {
        value["affected_plays"] = json!(plays);
    }
    if let Some(paths) = &updated.stranded_context_paths {
        value["stranded_context_paths"] = json!(paths);
    }
    value
}

/// Each kind of change an update made, keyed as the JSON output names it.
fn schema_update_counts(updated: &SchemaUpdateOutput) -> Vec<(&'static str, usize)> {
    // Destructured in full, so a field added to `SchemaUpdateOutput` fails to
    // compile here until the CLI says how it prints.
    let SchemaUpdateOutput {
        schema_id: _,
        success: _,
        fields_added,
        fields_removed,
        fields_renamed,
        field_values_added,
        relationships_added,
        relationships_removed,
        context_paths_added,
        context_paths_removed,
        stranded_context_paths: _,
        affected_plays: _,
    } = updated;

    [
        ("fields_added", fields_added),
        ("fields_removed", fields_removed),
        ("fields_renamed", fields_renamed),
        ("field_values_added", field_values_added),
        ("relationships_added", relationships_added),
        ("relationships_removed", relationships_removed),
        ("context_paths_added", context_paths_added),
        ("context_paths_removed", context_paths_removed),
    ]
    .into_iter()
    .filter_map(|(key, count)| Some((key, (*count)?)))
    .collect()
}

pub fn print_schema_updated(result_json: &str, json: bool) -> Result<()> {
    println!("{}", render_schema_updated(result_json, json)?);
    Ok(())
}

/// What `schema update` prints: the JSON object with `--json`, a short
/// summary without.
fn render_schema_updated(result_json: &str, json: bool) -> Result<String> {
    let updated: SchemaUpdateOutput =
        serde_json::from_str(result_json).context("daemon returned a malformed update result")?;
    if json {
        return Ok(serde_json::to_string_pretty(&schema_updated_to_json(
            &updated,
        ))?);
    }

    let mut lines = vec![format!("Updated schema {}", updated.schema_id)];
    for (key, count) in schema_update_counts(&updated) {
        lines.push(format!("    {}: {count}", key.replace('_', " ")));
    }
    for play in updated.affected_plays.iter().flatten() {
        lines.push(format!("affected play: {play}"));
    }
    for path in updated.stranded_context_paths.iter().flatten() {
        lines.push(format!(
            "stranded context path (no longer resolves; remove it with remove_context_paths): {path}"
        ));
    }
    Ok(lines.join("\n"))
}

/// Decode a schema read's JSON-encoded `SchemaNode`.
fn parse_schema(schema_json: &str) -> Result<SchemaNode> {
    serde_json::from_str(schema_json).context("daemon returned a malformed schema")
}

pub fn print_schema(schema_json: &str, json: bool) -> Result<()> {
    let schema = parse_schema(schema_json)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&schema_to_json(&schema))?
        );
    } else {
        write_human_schema(&schema);
    }
    Ok(())
}

pub fn print_schema_list(schemas_json: &[String], json: bool) -> Result<()> {
    let schemas = schemas_json
        .iter()
        .map(|schema_json| parse_schema(schema_json))
        .collect::<Result<Vec<_>>>()?;

    if json {
        let value = json!({
            "count": schemas.len(),
            "schemas": schemas.iter().map(schema_to_json).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if schemas.is_empty() {
        println!("No schemas returned (count: 0)");
        return Ok(());
    }

    println!("{} schema(s):", schemas.len());
    for (idx, schema) in schemas.iter().enumerate() {
        if idx > 0 {
            println!();
        }
        write_human_schema(schema);
    }
    Ok(())
}

fn write_human_schema(schema: &SchemaNode) {
    println!("id:              {}", schema.envelope.id);
    println!("name:            {}", schema.envelope.content);
    println!("version:         {}", schema.envelope.version);
    println!("core:            {}", schema.is_core);
    if schema.is_abstract {
        println!("abstract:        true");
    }
    if let Some(parent) = &schema.extends {
        println!("extends:         {}", parent);
    }
    if !schema.children.is_any() {
        println!("children:        {}", json!(schema.children));
    }
    if !schema.parent.is_any() {
        println!("parent:          {}", json!(schema.parent));
    }
    if let Some(template) = &schema.title_template {
        println!("title_template:  {}", template);
    }
    if let Some(template) = &schema.properties_header_summary_template {
        println!("summary_template: {}", template);
    }
    println!("fields:");
    if schema.fields.is_empty() {
        println!("    (none)");
    }
    for field in &schema.fields {
        println!("    {}: {}", field.name, field.field_type);
    }
    if !schema.relationships.is_empty() {
        println!("relationships:");
        for rel in &schema.relationships {
            println!(
                "    {} -> {} (reverse: {})",
                rel.name,
                rel.target_type.as_deref().unwrap_or("*"),
                rel.reverse_name
            );
        }
    }
    if !schema.context_paths.is_empty() {
        println!("context_paths:");
        for path in &schema.context_paths {
            println!("    {path}");
        }
    }
}

/// Parse the wire `properties` string into the flat API shape.
///
/// Storage nests properties under the schema id: `{"task": {"status": "open"}}`.
/// That nesting is a storage-layer concern and must never be observable on the
/// CLI surface — writes (`--property status=…`) and filters
/// (`{"property":"status"}`) already take bare field names, so reads emit bare
/// names too. A consumer can then `jq '.properties.status'` with no second
/// parse and no knowledge of the schema id.
///
/// The rule itself lives in `nodespace_types::flatten_namespaced_properties`,
/// shared with the `Node`-based frontend path so the two cannot drift. This
/// wrapper exists only because the CLI holds the gRPC `NodeData`, whose
/// `properties` is a JSON-encoded string (see `node_service.proto`).
///
/// Falls back to the raw string if it doesn't parse; in practice this branch is
/// unreachable because the daemon serializes via `serde_json::Value::to_string`,
/// but we'd rather degrade than panic if that contract ever breaks.
fn properties_to_json(node: &NodeData) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(&node.properties) {
        Ok(parsed) => nodespace_types::flatten_namespaced_properties(&parsed, &node.node_type),
        Err(_) => serde_json::Value::String(node.properties.clone()),
    }
}

/// Re-key one node of a `GetRelatedNodes` payload into the CLI's node shape.
///
/// `relationship get` is the one read path whose nodes the daemon serializes
/// itself, so they arrive in the frontend's typed shape: camelCase keys, an
/// injected `uri`, and for `task`/`ai-chat` type-specific fields promoted to
/// the top level. Every other command emits [`node_to_json`]'s snake_case
/// shape. Two shapes on one CLI surface is the same defect as two property
/// layouts — a consumer would have to know which command it called before it
/// could read `node_type` — so this maps the typed shape onto the CLI's.
///
/// Only the keys the CLI's own shape defines are re-keyed; promoted typed
/// fields and `uri` are dropped, since `properties` already carries the
/// promoted values and no other command emits a `uri`. Unrecognized keys pass
/// through untouched rather than being silently discarded.
pub fn related_node_to_json(node: &serde_json::Value) -> serde_json::Value {
    let Some(obj) = node.as_object() else {
        return node.clone();
    };

    // Every `Node` field that `rename_all = "camelCase"` spells differently
    // from the CLI's snake_case shape (see `nodespace_types::Node`).
    const RENAMES: &[(&str, &str)] = &[
        ("nodeType", "node_type"),
        ("createdAt", "created_at"),
        ("modifiedAt", "modified_at"),
        ("lifecycleStatus", "lifecycle_status"),
        ("mentionedIn", "mentioned_in"),
    ];
    // Keys that are node fields in their own right. Anything else at the top
    // level is a field the typed conversion promoted out of `properties`.
    //
    // The first eight are exactly what `node_to_json` emits, so the two shapes
    // agree on every key a consumer can rely on. The last three are `Node`
    // fields absent from the gRPC `NodeData` that `node_to_json` builds from:
    // they are listed so that IF the daemon ever populates them they stay
    // top-level rather than being misfiled as stored properties. Today the
    // store leaves `mentions`/`mentioned_in` empty and both are
    // `skip_serializing_if = "Vec::is_empty"`, so only `title` occurs in
    // practice.
    const CLI_KEYS: &[&str] = &[
        "id",
        "node_type",
        "content",
        "properties",
        "version",
        "lifecycle_status",
        "created_at",
        "modified_at",
        "title",
        "mentions",
        "mentioned_in",
    ];

    let mut out = serde_json::Map::with_capacity(obj.len());
    let mut promoted = serde_json::Map::new();
    for (key, value) in obj {
        // No other command emits a `uri`.
        if key == "uri" {
            continue;
        }
        let mapped = RENAMES
            .iter()
            .find(|(from, _)| from == key)
            .map(|(_, to)| (*to).to_string())
            .unwrap_or_else(|| key.clone());
        if CLI_KEYS.contains(&mapped.as_str()) {
            out.insert(mapped, value.clone());
        } else {
            promoted.insert(mapped, value.clone());
        }
    }

    // Fold promoted fields back under `properties`. The typed conversion moves
    // a core type's fields out of `properties` entirely (`task.due_date`
    // travels as the top-level `dueDate`), so they are restored under their
    // storage keys — the CLI's shape, which every other command emits and
    // which `--property` writes back. Those keys are then no longer
    // "promoted" leftovers.
    //
    // Any other unrecognized top-level key is folded in under its own name:
    // `node_to_typed_value` can synthesize a value that was never stored —
    // `task_node_to_value` defaults an absent `status` to "open" — so dropping
    // it outright would lose a field `properties` does not carry. Stored
    // values win: a key already in `properties` is the real one.
    let flat = nodespace_types::flat_properties_view(node);
    if let Some(node_type) = obj.get("nodeType").and_then(|v| v.as_str()) {
        for field in nodespace_types::promoted_fields(node_type) {
            promoted.remove(field.wire);
        }
    }
    out.insert("properties".to_string(), flat);
    if !promoted.is_empty() {
        let props = out
            .entry("properties".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let Some(props_obj) = props.as_object_mut() {
            for (key, value) in promoted {
                props_obj.entry(key).or_insert(value);
            }
        }
    }
    serde_json::Value::Object(out)
}

pub fn node_to_json(node: &NodeData) -> serde_json::Value {
    // properties is a JSON-encoded string on the wire — inline it as nested
    // JSON so scripts can `jq '.properties.foo'` without a second parse.
    let properties = properties_to_json(node);

    let mut value = json!({
        "id": node.id,
        "node_type": node.node_type,
        "content": node.content,
        "properties": properties,
        "version": node.version,
        "lifecycle_status": node.lifecycle_status,
        "created_at": node.created_at,
        "modified_at": node.modified_at,
    });
    // Only present when this node type actually gets a title (see
    // `NodeData.title`'s doc comment in node_service.proto) — omitted rather
    // than emitted `null`, matching `nodespace_types::Node`'s own
    // `skip_serializing_if = "Option::is_none"` so the CLI's JSON shape and
    // the Tauri app's JSON shape agree on presence, not just on the value
    // when present. A present-but-empty title (a title_template whose fields
    // are all still blank) DOES appear, as `""` — only absence is omitted.
    if let Some(title) = &node.title {
        value["title"] = json!(title);
    }
    // Only present when the request opted in (e.g. `search --include-content`)
    // — omitted rather than emitted empty, so scripts that don't ask for it
    // see the same node shape as every other command.
    if !node.markdown.is_empty() {
        value["markdown"] = json!(node.markdown);
    }
    value
}

/// Parse a conflict's `detail`/`resolution` wire strings (JSON-encoded, per
/// `node_service.proto`'s `ConflictRecord`) into real JSON, mirroring
/// `properties_to_json`'s reason for existing: a consumer should never have
/// to double-decode a nested JSON string.
pub fn conflict_to_json(record: &ConflictRecordProto) -> serde_json::Value {
    let detail: serde_json::Value =
        serde_json::from_str(&record.detail).unwrap_or_else(|_| json!(record.detail));
    let resolution = record
        .resolution
        .as_ref()
        .map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap_or_else(|_| json!(s)));

    json!({
        "id": record.id,
        "kind": record.kind,
        "node_ids": record.node_ids,
        "detail": detail,
        "status": record.status,
        "detected_at": record.detected_at,
        "detected_by": record.detected_by,
        "occurrences": record.occurrences,
        "last_seen_at": record.last_seen_at,
        "resolved_at": record.resolved_at,
        "resolution": resolution,
    })
}

pub fn print_conflict(record: &ConflictRecordProto, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&conflict_to_json(record))?
        );
    } else {
        write_human_conflict(record);
    }
    Ok(())
}

pub fn print_conflict_list(records: &[ConflictRecordProto], json: bool) -> Result<()> {
    if json {
        let value = json!({
            "count": records.len(),
            "conflicts": records.iter().map(conflict_to_json).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    if records.is_empty() {
        println!("No conflicts returned (count: 0)");
        return Ok(());
    }

    println!("{} conflict(s):", records.len());
    for (idx, record) in records.iter().enumerate() {
        if idx > 0 {
            println!();
        }
        write_human_conflict(record);
    }
    Ok(())
}

fn write_human_conflict(record: &ConflictRecordProto) {
    println!("id:              {}", record.id);
    println!("kind:            {}", record.kind);
    println!("status:          {}", record.status);
    println!("node_ids:        {}", record.node_ids.join(", "));
    println!("detected_at:     {}", record.detected_at);
    if let Some(by) = &record.detected_by {
        println!("detected_by:     {by}");
    }
    println!("occurrences:     {}", record.occurrences);
    println!("last_seen_at:    {}", record.last_seen_at);
    let detail: serde_json::Value =
        serde_json::from_str(&record.detail).unwrap_or_else(|_| json!(record.detail));
    println!("detail:          {detail}");
    if let Some(resolved_at) = &record.resolved_at {
        println!("resolved_at:     {resolved_at}");
    }
    if let Some(resolution) = &record.resolution {
        let resolution: serde_json::Value =
            serde_json::from_str(resolution).unwrap_or_else(|_| json!(resolution));
        println!("resolution:      {resolution}");
    }
}

pub fn print_merge_outcome(response: &MergeNodesResponse, json: bool) -> Result<()> {
    if json {
        let value = json!({
            "survivor_id": response.survivor_id,
            "loser_id": response.loser_id,
            "properties_merged": response.properties_merged,
            "edges_repointed": response.edges_repointed,
            "edges_dropped": response.edges_dropped,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!(
            "Merged {} into {}: {} propert{} merged, {} edge{} re-pointed, {} edge{} dropped",
            response.loser_id,
            response.survivor_id,
            response.properties_merged,
            if response.properties_merged == 1 {
                "y"
            } else {
                "ies"
            },
            response.edges_repointed,
            if response.edges_repointed == 1 {
                ""
            } else {
                "s"
            },
            response.edges_dropped,
            if response.edges_dropped == 1 { "" } else { "s" },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_node() -> NodeData {
        NodeData {
            id: "abc-123".into(),
            node_type: "text".into(),
            content: "hello".into(),
            properties: r#"{"foo":"bar","n":42}"#.into(),
            version: 7,
            lifecycle_status: "active".into(),
            created_at: "2026-05-17T12:00:00Z".into(),
            modified_at: "2026-05-17T12:00:01Z".into(),
            markdown: String::new(),
            title: None,
        }
    }

    #[test]
    fn node_to_json_inlines_properties_as_nested_object() {
        let json = node_to_json(&sample_node());
        // Scripts pipe `nodespace node get --json ID | jq '.properties.foo'`;
        // if this regresses to a JSON-encoded string they'd need a double
        // decode. Lock the inlined shape in.
        assert_eq!(json["properties"]["foo"], "bar");
        assert_eq!(json["properties"]["n"], 42);
        assert_eq!(json["id"], "abc-123");
        assert_eq!(json["version"], 7);
    }

    /// `NodeData.title` must reach `nodespace node get --json` /
    /// `nodespace query --json` output, not be silently dropped the way it
    /// was when `NodeData` carried no `title` field at all.
    #[test]
    fn node_to_json_includes_title_when_present() {
        let node = NodeData {
            title: Some("Michael Libio".into()),
            ..sample_node()
        };

        let json = node_to_json(&node);

        assert_eq!(json["title"], "Michael Libio");
    }

    /// Absence (this node type never gets a title) omits the key entirely —
    /// matching `nodespace_types::Node`'s own `skip_serializing_if =
    /// "Option::is_none"`, so a consumer scripting against either the CLI's
    /// JSON or the Tauri app's JSON sees the same presence rule.
    #[test]
    fn node_to_json_omits_title_when_absent() {
        let json = node_to_json(&sample_node());

        assert!(json.get("title").is_none());
    }

    /// A present-but-empty title (title_template fields all still blank) is a
    /// real, distinct value from absence and must not be collapsed into it.
    #[test]
    fn node_to_json_includes_an_empty_title_when_present_but_blank() {
        let node = NodeData {
            title: Some(String::new()),
            ..sample_node()
        };

        let json = node_to_json(&node);

        assert_eq!(json["title"], "");
    }

    #[test]
    fn node_to_json_falls_back_to_raw_string_for_malformed_properties() {
        let mut node = sample_node();
        node.properties = "{not valid json".into();
        let json = node_to_json(&node);
        // Unreachable in practice (daemon always serializes via serde), but
        // we degrade rather than panic if that contract ever breaks.
        assert_eq!(json["properties"], "{not valid json");
    }

    /// The exact shape a clean install returns for `query --type task --json`.
    /// A consumer reading `.properties.status` got `None` and could conclude
    /// "status unknown" about real data — the bug this flattening prevents.
    #[test]
    fn node_to_json_flattens_namespaced_properties_to_bare_field_names() {
        let mut node = sample_node();
        node.node_type = "task".into();
        node.properties = r#"{"task":{"_schema_version":1,"status":"open"}}"#.into();

        let json = node_to_json(&node);

        assert_eq!(json["properties"]["status"], "open");
        // The schema id must not be observable anywhere on the CLI surface.
        assert!(json["properties"].get("task").is_none());
    }

    /// `_`-prefixed keys are internal in the fallback branch too, where the
    /// properties carry no namespace for this node's type.
    #[test]
    fn node_to_json_hides_underscore_prefixed_internals_when_already_flat() {
        let mut node = sample_node();
        node.node_type = "text".into();
        node.properties = r#"{"_schema_version":1,"_seed":{"v":"x"},"note":"keep"}"#.into();

        let json = node_to_json(&node);

        assert_eq!(json["properties"]["note"], "keep");
        assert!(json["properties"].get("_schema_version").is_none());
        assert!(json["properties"].get("_seed").is_none());
    }

    /// Sibling namespaces (e.g. a seeded node's `_seed`) coexist with the type
    /// namespace at rest. Only the type's own fields are exposed.
    #[test]
    fn node_to_json_exposes_only_the_matching_type_namespace() {
        let mut node = sample_node();
        node.node_type = "skill".into();
        node.properties =
            r#"{"_seed":{"version":"abc"},"skill":{"description":"d","version":"1.0"}}"#.into();

        let json = node_to_json(&node);

        // `skill.version` wins; the `_seed` namespace never surfaces, so the
        // two `version` keys cannot collide in the flat output.
        assert_eq!(json["properties"]["version"], "1.0");
        assert_eq!(json["properties"]["description"], "d");
        assert!(json["properties"].get("_seed").is_none());
    }

    /// A user-defined type flattens by the same rule as a core type.
    #[test]
    fn node_to_json_flattens_user_defined_types() {
        let mut node = sample_node();
        node.node_type = "venue".into();
        node.properties = r#"{"venue":{"capacity":250,"_schema_version":2}}"#.into();

        let json = node_to_json(&node);

        assert_eq!(json["properties"]["capacity"], 250);
        assert!(json["properties"].get("venue").is_none());
        assert!(json["properties"].get("_schema_version").is_none());
    }

    /// A dormant namespace left by a previous type change is not exposed.
    #[test]
    fn node_to_json_hides_dormant_namespaces() {
        let mut node = sample_node();
        node.node_type = "task".into();
        node.properties = r#"{"task":{"status":"done"},"text":{"stale":"old"}}"#.into();

        let json = node_to_json(&node);

        assert_eq!(json["properties"]["status"], "done");
        assert!(json["properties"].get("text").is_none());
        assert!(json["properties"].get("stale").is_none());
    }

    /// Properties that are already flat (no namespace for this type) pass
    /// through, so untyped nodes keep working.
    #[test]
    fn node_to_json_passes_through_already_flat_properties() {
        let json = node_to_json(&sample_node());
        assert_eq!(json["properties"]["foo"], "bar");
        assert_eq!(json["properties"]["n"], 42);
    }

    /// An object-valued schema field inside the type namespace is a real value,
    /// not a namespace, and must survive flattening intact. Only siblings of the
    /// type namespace are filtered, and only in the already-flat fallback.
    #[test]
    fn node_to_json_preserves_object_valued_fields_inside_the_namespace() {
        let mut node = sample_node();
        node.node_type = "invoice".into();
        node.properties = r#"{"invoice":{"billing":{"city":"Berlin"},"amount":42}}"#.into();

        let json = node_to_json(&node);

        assert_eq!(json["properties"]["billing"]["city"], "Berlin");
        assert_eq!(json["properties"]["amount"], 42);
    }

    /// `relationship get`'s nodes arrive in the frontend's typed shape. They
    /// must come out matching every other command's, or a consumer would have
    /// to know which command it called before it could read `node_type`.
    #[test]
    fn related_node_is_rekeyed_to_the_cli_node_shape() {
        let typed = serde_json::json!({
            "id": "n1",
            "nodeType": "ticket",
            "content": "target",
            "title": "A ticket",
            "properties": {"severity": "high"},
            "version": 2,
            "createdAt": "2026-05-17T12:00:00Z",
            "modifiedAt": "2026-05-17T12:00:01Z",
            "uri": "nodespace://n1",
        });

        let out = related_node_to_json(&typed);

        assert_eq!(out["node_type"], "ticket");
        assert_eq!(out["created_at"], "2026-05-17T12:00:00Z");
        assert_eq!(out["modified_at"], "2026-05-17T12:00:01Z");
        assert_eq!(out["properties"]["severity"], "high");
        // camelCase spellings and the injected `uri` are not part of the CLI shape.
        assert!(out.get("nodeType").is_none());
        assert!(out.get("createdAt").is_none());
        assert!(out.get("uri").is_none());
    }

    /// Promoted typed fields belong under `properties`, not beside it — one
    /// shape, and the stored value wins over the promoted copy.
    #[test]
    fn related_node_folds_promoted_typed_fields_into_properties() {
        let typed = serde_json::json!({
            "id": "t1",
            "nodeType": "task",
            "content": "Buy groceries",
            "properties": {"status": "open"},
            "status": "open",
            "priority": "high",
            "uri": "nodespace://t1",
        });

        let out = related_node_to_json(&typed);

        assert_eq!(out["properties"]["status"], "open");
        assert_eq!(out["properties"]["priority"], "high");
        assert!(out.get("status").is_none());
        assert!(out.get("priority").is_none());
    }

    /// A core type's promoted fields come back under their storage keys — the
    /// spelling every other CLI command emits and `--property` writes — not
    /// the camelCase wire key the typed conversion moved them to.
    #[test]
    fn related_node_restores_promoted_fields_under_storage_keys() {
        let typed = serde_json::json!({
            "id": "p1",
            "nodeType": "person",
            "content": "",
            "properties": {"custom:team": "Core"},
            "firstName": "Ada",
            "email": "ada@example.com",
        });

        let out = related_node_to_json(&typed);

        assert_eq!(
            out["properties"],
            serde_json::json!({
                "first_name": "Ada",
                "email": "ada@example.com",
                "custom:team": "Core"
            })
        );
        assert!(out["properties"].get("firstName").is_none());
        assert!(out.get("firstName").is_none());
    }

    /// `task_node_to_value` defaults an absent `status` to "open" via
    /// `unwrap_or_default()` — it is not an `Option`, so unlike the other
    /// promoted fields it is serialized even when nothing was stored. Dropping
    /// promoted keys outright therefore lost it, and `relationship get`
    /// reported no status where `node get` reported "open".
    #[test]
    fn related_node_keeps_a_synthesized_status_absent_from_properties() {
        let typed = serde_json::json!({
            "id": "t1",
            "nodeType": "task",
            "content": "Buy groceries",
            "properties": {"priority": "high"},
            "status": "open",
            "uri": "nodespace://t1",
        });

        let out = related_node_to_json(&typed);

        assert_eq!(
            out["properties"]["status"], "open",
            "a promoted field `properties` does not carry must not be lost"
        );
        assert_eq!(out["properties"]["priority"], "high");
    }

    /// A node with inbound mentions carries `mentionedIn`; every key in the
    /// CLI's shape is snake_case, so it must not survive as camelCase.
    #[test]
    fn related_node_rekeys_mentioned_in() {
        let typed = serde_json::json!({
            "id": "n1",
            "nodeType": "text",
            "content": "hello",
            "properties": {},
            "mentions": ["other"],
            "mentionedIn": [{"id": "src", "nodeType": "text"}],
        });

        let out = related_node_to_json(&typed);

        assert!(out.get("mentioned_in").is_some());
        assert!(out.get("mentionedIn").is_none());
        assert!(out.get("mentions").is_some());
        // Neither is a promoted property.
        assert!(out["properties"].get("mentioned_in").is_none());
    }

    /// A schema reached through a relationship is a node like any other: it
    /// is re-keyed to the CLI's node shape, with its stored definition under
    /// `properties`. The typed schema is read with `schema get`.
    #[test]
    fn related_schema_node_is_rekeyed_like_any_node() {
        let schema = serde_json::json!({
            "id": "invoice",
            "nodeType": "schema",
            "content": "Invoice",
            "properties": {"isCore": false, "schemaVersion": 1, "fields": []},
            "createdAt": "2026-05-17T12:00:00Z",
        });

        let out = related_node_to_json(&schema);

        assert_eq!(out["node_type"], "schema");
        assert_eq!(out["created_at"], "2026-05-17T12:00:00Z");
        assert_eq!(out["properties"]["schemaVersion"], 1);
        assert!(out.get("nodeType").is_none());
    }

    /// `fields` is not a reserved property name: a user-defined type may
    /// declare one, and it is re-keyed like any other node.
    #[test]
    fn related_node_rekeys_a_user_type_that_has_a_fields_property() {
        let typed = serde_json::json!({
            "id": "f1",
            "nodeType": "form",
            "content": "Signup form",
            "properties": {"fields": ["name", "email"]},
            "createdAt": "2026-05-17T12:00:00Z",
        });

        let out = related_node_to_json(&typed);

        assert_eq!(out["node_type"], "form");
        assert_eq!(out["created_at"], "2026-05-17T12:00:00Z");
        assert_eq!(out["properties"]["fields"][0], "name");
    }

    /// A node whose only properties are internal renders as empty, not as a
    /// leaked namespace — and human mode omits the line entirely.
    #[test]
    fn flatten_yields_empty_object_when_only_internals_present() {
        let flat = properties_to_json(&NodeData {
            node_type: "task".into(),
            properties: r#"{"task":{"_schema_version":1}}"#.into(),
            ..sample_node()
        });
        assert_eq!(flat, serde_json::json!({}));
    }

    fn previewed() -> DeleteNodeResponse {
        DeleteNodeResponse {
            node_id: "abc123".into(),
            existed: true,
            deleted_count: 0,
            title: "Q3 planning".into(),
            node_type: "text".into(),
            version: 7,
            descendant_count: 4,
        }
    }

    #[test]
    fn delete_preview_json_is_the_shape_agents_parse() {
        let command = delete_confirm_command(&previewed(), &[]);
        assert_eq!(
            delete_preview_json(&previewed(), &command),
            serde_json::json!({
                "node_id": "abc123",
                "existed": true,
                "deleted": false,
                "title": "Q3 planning",
                "node_type": "text",
                "version": 7,
                "descendant_count": 4,
                "confirm_command": "nodespace node delete abc123 --version 7 --descendants 4",
            })
        );
    }

    #[test]
    fn delete_confirm_command_keeps_the_database_it_previewed() {
        let routing = [
            "--socket".to_string(),
            "/tmp/my dir/ns.sock".to_string(),
            "--database".to_string(),
            "work".to_string(),
        ];
        assert_eq!(
            delete_confirm_command(&previewed(), &routing),
            "nodespace --socket '/tmp/my dir/ns.sock' --database work \
             node delete abc123 --version 7 --descendants 4"
        );
    }

    fn sample_schema_json() -> String {
        serde_json::json!({
            "id": "invoice",
            "nodeType": "schema",
            "content": "Invoice",
            "version": 3,
            "createdAt": "2026-05-17T12:00:00Z",
            "modifiedAt": "2026-05-18T12:00:00Z",
            "properties": {},
            "lifecycleStatus": "active",
            "isCore": false,
            "abstract": true,
            "extends": "document",
            "children": { "rule": "none" },
            "schemaVersion": 2,
            "fields": [{ "name": "amount", "type": "number", "friendlyName": "Amount" }],
            "relationships": [{
                "name": "billed_to",
                "targetType": "customer",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "invoices",
                "reverseCardinality": "many"
            }],
            "titleTemplate": "{amount}",
            "propertiesHeaderSummaryTemplate": "{amount}"
        })
        .to_string()
    }

    /// `schema get` reads a schema under the keys `schema create` and
    /// `schema update` write it with: `title_template` is one top-level key
    /// on both sides, and nothing is nested under `properties`.
    #[test]
    fn schema_json_uses_the_keys_the_schema_params_take() {
        let schema = parse_schema(&sample_schema_json()).unwrap();
        let out = schema_to_json(&schema);

        assert_eq!(out["id"], "invoice");
        assert_eq!(out["node_type"], "schema");
        assert_eq!(out["content"], "Invoice");
        assert_eq!(out["version"], 3);
        assert_eq!(out["is_core"], false);
        assert_eq!(out["schema_version"], 2);
        assert_eq!(out["abstract"], true);
        assert_eq!(out["extends"], "document");
        assert_eq!(out["children"], serde_json::json!({ "rule": "none" }));
        assert_eq!(out["title_template"], "{amount}");
        assert_eq!(out["properties_header_summary_template"], "{amount}");
        assert_eq!(out["fields"][0]["friendlyName"], "Amount");
        assert_eq!(out["relationships"][0]["reverseName"], "invoices");

        for absent in [
            "properties",
            "titleTemplate",
            "isCore",
            "nodeType",
            "parent",
        ] {
            assert!(out.get(absent).is_none(), "`{absent}` must not appear");
        }

        // Every create parameter a schema carries is read back under the
        // same name.
        let params: nodespace_types::CreateSchemaParams =
            serde_json::from_value(serde_json::json!({
                "name": out["content"],
                "fields": out["fields"],
                "relationships": out["relationships"],
                "extends": out["extends"],
                "abstract": out["abstract"],
                "children": out["children"],
                "title_template": out["title_template"],
                "properties_header_summary_template": out["properties_header_summary_template"],
            }))
            .expect("the read shape feeds the create parameters");
        assert_eq!(params.title_template.as_deref(), Some("{amount}"));
    }

    #[test]
    fn schema_json_omits_what_a_schema_does_not_declare() {
        let schema = SchemaNode::new("note", "Note");
        let out = schema_to_json(&schema);

        for absent in [
            "abstract",
            "extends",
            "children",
            "parent",
            "title_template",
            "properties_header_summary_template",
        ] {
            assert!(out.get(absent).is_none(), "`{absent}` must not appear");
        }
        assert_eq!(out["fields"], serde_json::json!([]));
        assert_eq!(out["relationships"], serde_json::json!([]));
    }

    #[test]
    fn a_malformed_schema_is_an_error_not_an_empty_one() {
        assert!(parse_schema("{\"id\": 1}").is_err());
    }

    /// `schema create` names the schema's own values as `schema get` does, so
    /// one set of keys reads both.
    #[test]
    fn a_created_schema_prints_under_the_keys_a_schema_read_uses() {
        let created: CreateSchemaOutput = serde_json::from_value(serde_json::json!({
            "schemaId": "invoice",
            "isCore": false,
            "version": 1,
            "description": "A bill",
            "fields": [{ "name": "amount", "type": "number" }],
            "extends": "document",
            "warnings": ["field 'status' shadows a core property"],
        }))
        .expect("the daemon's create result");

        let value = schema_created_to_json(&created);
        assert_eq!(value["id"], "invoice");
        assert_eq!(value["is_core"], false);
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["description"], "A bill");
        assert_eq!(value["fields"][0]["name"], "amount");
        assert_eq!(value["relationships"], serde_json::json!([]));
        assert_eq!(value["extends"], "document");
        assert_eq!(
            value["warnings"][0],
            "field 'status' shadows a core property"
        );
        for wire_key in ["schemaId", "isCore", "version"] {
            assert!(value.get(wire_key).is_none(), "{wire_key} leaked");
        }
    }

    /// Both schema writes print JSON only when asked: the flag picks between
    /// the snake_case object and the summary.
    #[test]
    fn schema_writes_print_json_only_with_the_json_flag() {
        let created = r#"{"schemaId":"invoice","isCore":false,"version":1,"description":"A bill","fields":[]}"#;
        let as_json: Value =
            serde_json::from_str(&render_schema_created(created, true).expect("json"))
                .expect("--json prints JSON");
        assert_eq!(as_json["id"], "invoice");
        assert_eq!(
            render_schema_created(created, false).expect("summary"),
            "Created schema invoice\nfields:\n    (none)"
        );

        let updated = r#"{"schemaId":"invoice","success":true,"fieldsAdded":2}"#;
        let as_json: Value =
            serde_json::from_str(&render_schema_updated(updated, true).expect("json"))
                .expect("--json prints JSON");
        assert_eq!(as_json["fields_added"], 2);
        assert_eq!(
            render_schema_updated(updated, false).expect("summary"),
            "Updated schema invoice\n    fields added: 2"
        );
    }

    #[test]
    fn an_updated_schema_reports_only_the_changes_it_made() {
        let updated: SchemaUpdateOutput = serde_json::from_value(serde_json::json!({
            "schemaId": "invoice",
            "success": true,
            "fieldsAdded": 2,
            "fieldValuesAdded": 1,
        }))
        .expect("the daemon's update result");

        assert_eq!(
            schema_updated_to_json(&updated),
            serde_json::json!({
                "id": "invoice",
                "success": true,
                "fields_added": 2,
                "field_values_added": 1,
            })
        );
    }
}

/// Exit status when stdout's reader went away: 128 + SIGPIPE(13), what a shell
/// reports for a process killed by SIGPIPE.
pub const BROKEN_PIPE_EXIT_CODE: i32 = 141;

/// True when `message` is the panic text Rust's `print!`/`println!` raise for a
/// stdout write failure caused by a closed pipe (EPIPE).
fn is_stdout_broken_pipe(message: &str) -> bool {
    message.starts_with("failed printing to stdout:")
        && (message.contains("Broken pipe") || message.contains("The pipe is being closed"))
}

/// Exit quietly with [`BROKEN_PIPE_EXIT_CODE`] when `error` is a stdout
/// broken pipe that a command returned as `Err` (writers over an `impl Write`
/// such as `skill guidance`), rather than raised as a `println!` panic.
pub fn exit_if_broken_pipe(error: &anyhow::Error) {
    let broken = error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
    });
    if broken {
        std::process::exit(BROKEN_PIPE_EXIT_CODE);
    }
}

/// Make `nodespace ... | head` exit quietly when the reader closes early.
///
/// Rust ignores SIGPIPE, so a write to a closed pipe returns EPIPE and
/// `println!` panics with "failed printing to stdout: Broken pipe". Every
/// command prints through `println!`, so this one panic hook is the single
/// place that turns that case into a silent exit with
/// [`BROKEN_PIPE_EXIT_CODE`]. Any other panic, including other stdout write
/// errors, still goes to the previous hook unchanged.
pub fn install_broken_pipe_handler() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied());
        if message.is_some_and(is_stdout_broken_pipe) {
            std::process::exit(BROKEN_PIPE_EXIT_CODE);
        }
        previous(info);
    }));
}

#[cfg(test)]
mod broken_pipe_tests {
    use super::is_stdout_broken_pipe;

    #[test]
    fn recognises_only_the_stdout_broken_pipe_panic() {
        assert!(is_stdout_broken_pipe(
            "failed printing to stdout: Broken pipe (os error 32)"
        ));
        assert!(!is_stdout_broken_pipe(
            "failed printing to stdout: No space left on device (os error 28)"
        ));
        assert!(!is_stdout_broken_pipe(
            "failed printing to stderr: Broken pipe (os error 32)"
        ));
        assert!(!is_stdout_broken_pipe("something else"));
    }
}
