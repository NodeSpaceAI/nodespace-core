//! Output formatters for CLI subcommands.
//!
//! Human-readable mode emits a stable, label-prefixed layout intended for
//! interactive use. JSON mode emits the proto-as-JSON representation so the
//! output is unambiguous and scriptable.

use anyhow::Result;
use nodespace_daemon::nodespace::{
    ConflictRecord as ConflictRecordProto, DeleteNodeResponse, MergeNodesResponse, NodeListResponse,
};
use nodespace_daemon::NodeData;
use serde_json::json;

pub fn print_node(node: &NodeData, json: bool) -> Result<()> {
    if json {
        let value = node_to_json(node);
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        write_human_node(node);
    }
    Ok(())
}

pub fn print_delete(response: &DeleteNodeResponse, json: bool) -> Result<()> {
    if json {
        let value = json!({
            "node_id": response.node_id,
            "existed": response.existed,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else if response.existed {
        println!("Deleted node {}", response.node_id);
    } else {
        println!("Node {} did not exist (no-op)", response.node_id);
    }
    Ok(())
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

fn write_human_node(node: &NodeData) {
    println!("id:              {}", node.id);
    println!("type:            {}", node.node_type);
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
        println!("properties:      {}", properties);
    }
    println!("content:");
    for line in node.content.lines() {
        println!("    {}", line);
    }
    if node.content.is_empty() {
        println!("    (empty)");
    }
    if !node.markdown.is_empty() {
        println!("markdown:");
        for line in node.markdown.lines() {
            println!("    {}", line);
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

    // `schema` nodes reach us reshaped by `SchemaNode::from_node`, which drops
    // `nodeType`/`title`/`properties` wholesale. Nothing can be recovered from
    // this end, so pass them through rather than emitting a half-mapped node
    // that looks like the CLI shape but is missing its fields.
    //
    // Identify them by the ABSENCE of `nodeType`, which every node shape
    // carries and `SchemaNode` does not. Keying off a field it happens to have
    // (`fields`, `schemaVersion`) would misfire on a user-defined type that
    // legitimately declares a field by that name — `fields` is not reserved.
    if !obj.contains_key("nodeType") {
        return node.clone();
    }

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

    // Fold promoted fields back under `properties`. `node_to_typed_value` can
    // synthesize a value that was never stored — `task_node_to_value` defaults
    // an absent `status` to "open" — so dropping these outright would lose a
    // field that `properties` does not carry. Stored values win: a key already
    // in `properties` is the real one.
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

    /// A `schema` node reaches us reshaped by `SchemaNode::from_node`, which
    /// has already dropped `nodeType`/`title`/`properties`. Nothing can be
    /// recovered here, so pass it through rather than emit a half-mapped node.
    #[test]
    fn related_node_passes_schema_nodes_through_untouched() {
        let schema = serde_json::json!({
            "id": "s1",
            "name": "Ticket",
            "fields": [{"name": "severity"}],
            "schemaVersion": 1,
        });

        let out = related_node_to_json(&schema);

        assert_eq!(out, schema);
    }

    /// The passthrough keys off the ABSENCE of `nodeType`, not the presence of
    /// `fields`: `fields` is not a reserved property name, so a user-defined
    /// type may legitimately declare one and must still be re-keyed.
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
}
