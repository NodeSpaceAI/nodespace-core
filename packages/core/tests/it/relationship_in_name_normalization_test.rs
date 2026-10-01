//! A relationship written through an `in`-direction declaration's own name is
//! stored as the forward edge it is the far end's view of.
//!
//! Edges are stored once: `relationship_type` is the forward (`out`) name,
//! `in_node` the source, `out_node` the target. `adr` declares `supersedes`
//! (out) and `superseded_by` (in, `reverseName: supersedes`), so
//! `old --superseded_by--> new` is `new --supersedes--> old`. Every write path
//! (create, update, delete) rewrites the `in` spelling to that forward shape,
//! so one logical edge has one storage shape and the forward declaration's
//! cardinality applies to it.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::Node,
    ops::rel_ops::{self, GetRelatedInput},
    schema::{handle_create_schema, handle_update_schema},
    services::{NodeService, StoredEdge},
};
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

async fn create_test_service() -> Result<(Arc<NodeService>, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let mut store = Arc::new(SqliteStore::new(db_path).await?);
    let node_service = Arc::new(NodeService::new(&mut store).await?);
    Ok((node_service, temp_dir))
}

async fn make_adr(svc: &NodeService, id: &str) -> Result<()> {
    svc.create_node(Node::new_with_id(
        id.to_string(),
        "in_norm_adr".to_string(),
        format!("{id} content"),
        json!({}),
    ))
    .await?;
    Ok(())
}

/// `supersedes` is one-to-one: an ADR supersedes at most one other, and is
/// superseded by at most one. The pair carries an enum edge field so the
/// update path's validation is observable.
async fn create_adr_schema(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "in_norm_adr",
            "fields": [],
            "relationships": [
                {
                    "name": "supersedes",
                    "targetType": "in_norm_adr",
                    "direction": "out",
                    "cardinality": "one",
                    "reverseName": "superseded_by",
                    "reverseCardinality": "one",
                    "edgeFields": [{
                        "name": "reason",
                        "type": "enum",
                        "coreValues": [
                            { "value": "revised", "label": "Revised" },
                            { "value": "reversed", "label": "Reversed" }
                        ]
                    }]
                },
                {
                    "name": "superseded_by",
                    "targetType": "in_norm_adr",
                    "direction": "in",
                    "cardinality": "one",
                    "reverseName": "supersedes",
                    "reverseCardinality": "one"
                }
            ]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("adr schema: {e}"))?;
    Ok(())
}

/// Every stored edge leaving one of `ids` under either spelling, as
/// `(in_node, out_node, relationship_type)`. `get_related_nodes` queries the
/// store by the literal `relationship_type`, so this sees the raw rows.
async fn stored_edges(svc: &NodeService, ids: &[&str]) -> Result<Vec<(String, String, String)>> {
    let mut stored = Vec::new();
    for id in ids {
        for name in ["supersedes", "superseded_by"] {
            for target in related_ids(svc, id, name, "out").await? {
                stored.push((id.to_string(), target, name.to_string()));
            }
        }
    }
    stored.sort();
    Ok(stored)
}

async fn related_ids(svc: &NodeService, id: &str, name: &str, dir: &str) -> Result<Vec<String>> {
    Ok(svc
        .get_related_nodes(id, name, dir)
        .await?
        .into_iter()
        .map(|n| n.id)
        .collect())
}

#[tokio::test]
async fn write_through_in_name_stores_the_forward_edge() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    make_adr(&svc, "ac012a23-2b8e-5fdc-9aca-df129fe90625").await?;
    make_adr(&svc, "15881b3b-b2c4-5713-8dc5-03428d288eb7").await?;

    svc.create_relationship(
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        "superseded_by",
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
        json!({}),
    )
    .await?;

    assert_eq!(
        stored_edges(
            &svc,
            &[
                "ac012a23-2b8e-5fdc-9aca-df129fe90625",
                "15881b3b-b2c4-5713-8dc5-03428d288eb7"
            ]
        )
        .await?,
        vec![(
            "15881b3b-b2c4-5713-8dc5-03428d288eb7".into(),
            "ac012a23-2b8e-5fdc-9aca-df129fe90625".into(),
            "supersedes".into()
        )],
        "same row `create_relationship(new, supersedes, old)` stores"
    );
    assert_eq!(
        related_ids(
            &svc,
            "15881b3b-b2c4-5713-8dc5-03428d288eb7",
            "supersedes",
            "out"
        )
        .await?,
        ["ac012a23-2b8e-5fdc-9aca-df129fe90625"]
    );
    assert_eq!(
        related_ids(
            &svc,
            "ac012a23-2b8e-5fdc-9aca-df129fe90625",
            "supersedes",
            "in"
        )
        .await?,
        ["15881b3b-b2c4-5713-8dc5-03428d288eb7"]
    );

    // Writing the forward spelling of the same edge is an idempotent no-op.
    svc.create_relationship(
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
        "supersedes",
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        json!({}),
    )
    .await?;
    assert_eq!(
        stored_edges(
            &svc,
            &[
                "ac012a23-2b8e-5fdc-9aca-df129fe90625",
                "15881b3b-b2c4-5713-8dc5-03428d288eb7"
            ]
        )
        .await?
        .len(),
        1
    );
    Ok(())
}

/// A write through the `in` name reports the edge as stored, not as given —
/// the same orientation a later write's `replaced` entries use for it, so a
/// caller can tell when that later write evicted it.
#[tokio::test]
async fn write_through_in_name_reports_the_stored_edge() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    for id in [
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        "46b24f8a-39b8-5605-a0fa-dd268865d77d",
        "a123b714-54cc-528d-bbbe-5b166e79416c",
    ] {
        make_adr(&svc, id).await?;
    }

    let first = rel_ops::create_relationship(
        &svc,
        rel_ops::CreateRelInput {
            source_id: "ac012a23-2b8e-5fdc-9aca-df129fe90625".into(),
            relationship_name: "superseded_by".into(),
            target_id: "46b24f8a-39b8-5605-a0fa-dd268865d77d".into(),
            edge_data: None,
        },
    )
    .await?;
    assert_eq!(
        (
            first.source_id.as_str(),
            first.relationship_name.as_str(),
            first.target_id.as_str()
        ),
        (
            "46b24f8a-39b8-5605-a0fa-dd268865d77d",
            "supersedes",
            "ac012a23-2b8e-5fdc-9aca-df129fe90625"
        )
    );

    let second = svc
        .create_relationship(
            "ac012a23-2b8e-5fdc-9aca-df129fe90625",
            "superseded_by",
            "a123b714-54cc-528d-bbbe-5b166e79416c",
            json!({}),
        )
        .await?;
    assert_eq!(
        second.edge,
        StoredEdge {
            source_id: "a123b714-54cc-528d-bbbe-5b166e79416c".into(),
            relationship_name: "supersedes".into(),
            target_id: "ac012a23-2b8e-5fdc-9aca-df129fe90625".into(),
        }
    );
    assert_eq!(
        second.replaced,
        vec![StoredEdge {
            source_id: first.source_id,
            relationship_name: first.relationship_name,
            target_id: first.target_id,
        }],
        "the eviction names the first write's edge exactly as that write reported it"
    );
    Ok(())
}

/// `reverse_cardinality: one` on `supersedes` means `old` is superseded by at
/// most one ADR. A second write through the `in` name replaces the first
/// superseder rather than adding a second edge.
#[tokio::test]
async fn write_through_in_name_enforces_reverse_cardinality() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    for id in [
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        "46b24f8a-39b8-5605-a0fa-dd268865d77d",
        "a123b714-54cc-528d-bbbe-5b166e79416c",
    ] {
        make_adr(&svc, id).await?;
    }

    svc.create_relationship(
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        "superseded_by",
        "46b24f8a-39b8-5605-a0fa-dd268865d77d",
        json!({}),
    )
    .await?;
    svc.create_relationship(
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        "superseded_by",
        "a123b714-54cc-528d-bbbe-5b166e79416c",
        json!({}),
    )
    .await?;

    assert_eq!(
        stored_edges(
            &svc,
            &[
                "ac012a23-2b8e-5fdc-9aca-df129fe90625",
                "46b24f8a-39b8-5605-a0fa-dd268865d77d",
                "a123b714-54cc-528d-bbbe-5b166e79416c"
            ]
        )
        .await?,
        vec![(
            "a123b714-54cc-528d-bbbe-5b166e79416c".into(),
            "ac012a23-2b8e-5fdc-9aca-df129fe90625".into(),
            "supersedes".into()
        )]
    );
    Ok(())
}

/// `cardinality: one` on `supersedes` means `new` supersedes at most one ADR.
/// A write through the `in` name from a second target replaces `new`'s prior
/// forward edge.
#[tokio::test]
async fn write_through_in_name_enforces_forward_cardinality() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    for id in [
        "c1480ac4-713a-590b-a31a-974e04e77598",
        "4f291177-bef2-5b10-80ba-8b6aa7822fc4",
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
    ] {
        make_adr(&svc, id).await?;
    }

    svc.create_relationship(
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
        "supersedes",
        "c1480ac4-713a-590b-a31a-974e04e77598",
        json!({}),
    )
    .await?;
    svc.create_relationship(
        "4f291177-bef2-5b10-80ba-8b6aa7822fc4",
        "superseded_by",
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
        json!({}),
    )
    .await?;

    assert_eq!(
        stored_edges(
            &svc,
            &[
                "c1480ac4-713a-590b-a31a-974e04e77598",
                "4f291177-bef2-5b10-80ba-8b6aa7822fc4",
                "15881b3b-b2c4-5713-8dc5-03428d288eb7"
            ]
        )
        .await?,
        vec![(
            "15881b3b-b2c4-5713-8dc5-03428d288eb7".into(),
            "4f291177-bef2-5b10-80ba-8b6aa7822fc4".into(),
            "supersedes".into()
        )]
    );
    Ok(())
}

#[tokio::test]
async fn update_and_delete_resolve_the_in_name() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    make_adr(&svc, "ac012a23-2b8e-5fdc-9aca-df129fe90625").await?;
    make_adr(&svc, "15881b3b-b2c4-5713-8dc5-03428d288eb7").await?;

    svc.create_relationship(
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        "superseded_by",
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
        json!({}),
    )
    .await?;

    // The forward declaration's edge fields validate an update through the
    // `in` name, and the update lands on the forward row.
    let bad = svc
        .update_relationship_properties(
            "ac012a23-2b8e-5fdc-9aca-df129fe90625",
            "superseded_by",
            "15881b3b-b2c4-5713-8dc5-03428d288eb7",
            json!({"reason": "nope"}),
        )
        .await;
    assert!(bad.is_err(), "enum edge field must be validated");
    svc.update_relationship_properties(
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        "superseded_by",
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
        json!({"reason": "revised"}),
    )
    .await?;
    let edges = svc
        .get_related_nodes_with_edges("15881b3b-b2c4-5713-8dc5-03428d288eb7", "supersedes", "out")
        .await?;
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].1["reason"], "revised");

    svc.delete_relationship(
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        "superseded_by",
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
    )
    .await?;
    assert!(stored_edges(
        &svc,
        &[
            "ac012a23-2b8e-5fdc-9aca-df129fe90625",
            "15881b3b-b2c4-5713-8dc5-03428d288eb7"
        ]
    )
    .await?
    .is_empty());
    Ok(())
}

/// A forward-written edge is removable through the `in` name too — the two
/// spellings address one edge.
#[tokio::test]
async fn forward_written_edge_deletes_through_in_name() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    make_adr(&svc, "ac012a23-2b8e-5fdc-9aca-df129fe90625").await?;
    make_adr(&svc, "15881b3b-b2c4-5713-8dc5-03428d288eb7").await?;

    svc.create_relationship(
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
        "supersedes",
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        json!({}),
    )
    .await?;
    svc.delete_relationship(
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        "superseded_by",
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
    )
    .await?;

    assert!(stored_edges(
        &svc,
        &[
            "ac012a23-2b8e-5fdc-9aca-df129fe90625",
            "15881b3b-b2c4-5713-8dc5-03428d288eb7"
        ]
    )
    .await?
    .is_empty());
    Ok(())
}

/// Reading through the node's own `in` declaration traverses the forward
/// edge inbound, whichever spelling wrote it.
#[tokio::test]
async fn read_through_in_name_finds_the_forward_edge() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await?;
    make_adr(&svc, "ac012a23-2b8e-5fdc-9aca-df129fe90625").await?;
    make_adr(&svc, "15881b3b-b2c4-5713-8dc5-03428d288eb7").await?;

    svc.create_relationship(
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
        "supersedes",
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        json!({}),
    )
    .await?;

    let out = rel_ops::get_related_nodes(
        &svc,
        GetRelatedInput {
            node_id: "ac012a23-2b8e-5fdc-9aca-df129fe90625".to_string(),
            relationship_name: "superseded_by".to_string(),
            direction: "out".to_string(),
        },
    )
    .await?;
    assert_eq!(out.relationship_name, "supersedes");
    assert_eq!(out.direction, "in");
    assert_eq!(out.count, 1);
    assert_eq!(
        out.related_nodes[0]["id"],
        "15881b3b-b2c4-5713-8dc5-03428d288eb7"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// A pair across two types, so a swap that validated or narrowed against the
// wrong end cannot pass by both ends sharing one type.
// ---------------------------------------------------------------------------

async fn make_node(svc: &NodeService, id: &str, node_type: &str) -> Result<()> {
    svc.create_node(Node::new_with_id(
        id.to_string(),
        node_type.to_string(),
        format!("{id} content"),
        json!({}),
    ))
    .await?;
    Ok(())
}

/// `in_norm_person.approves` (out) targets `in_norm_doc`, which names the
/// same edge `approved_by` (in). `in_norm_memo` declares its own `approves`
/// toward docs — the same stored forward name from a different source type.
async fn create_cross_type_schemas(svc: &Arc<NodeService>) -> Result<()> {
    // Each end's declaration names the other as its target, so the doc
    // exists first and gains its `in` declaration once the person does.
    handle_create_schema(svc, json!({ "name": "in_norm_doc", "fields": [] }))
        .await
        .map_err(|e| anyhow::anyhow!("doc schema: {e}"))?;
    for source in ["in_norm_person", "in_norm_memo"] {
        handle_create_schema(
            svc,
            json!({
                "name": source,
                "fields": [],
                "relationships": [{
                    "name": "approves",
                    "targetType": "in_norm_doc",
                    "direction": "out",
                    "cardinality": "many",
                    "reverseName": "approved_by",
                    "reverseCardinality": "many"
                }]
            }),
        )
        .await
        .map_err(|e| anyhow::anyhow!("{source} schema: {e}"))?;
    }
    handle_update_schema(
        svc,
        json!({
            "schema_id": "in_norm_doc",
            "add_relationships": [{
                "name": "approved_by",
                "targetType": "in_norm_person",
                "direction": "in",
                "cardinality": "many",
                "reverseName": "approves",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("doc approved_by: {e}"))?;
    make_node(svc, "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167", "in_norm_doc").await?;
    make_node(
        svc,
        "99e6a416-b162-589d-97e9-af6fa8004f05",
        "in_norm_person",
    )
    .await?;
    make_node(svc, "d7ac97fb-f0b6-5af0-bc46-80281a91c768", "in_norm_memo").await?;
    Ok(())
}

#[tokio::test]
async fn cross_type_write_through_in_name_stores_forward_edge() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_cross_type_schemas(&svc).await?;

    svc.create_relationship(
        "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167",
        "approved_by",
        "99e6a416-b162-589d-97e9-af6fa8004f05",
        json!({}),
    )
    .await?;

    assert_eq!(
        related_ids(
            &svc,
            "99e6a416-b162-589d-97e9-af6fa8004f05",
            "approves",
            "out"
        )
        .await?,
        ["ee5ab1a7-1de8-531f-a862-9e9c1b2a2167"]
    );
    assert!(related_ids(
        &svc,
        "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167",
        "approved_by",
        "out"
    )
    .await?
    .is_empty());
    Ok(())
}

/// After the swap the far end is the forward source, so the forward
/// declaration is resolved on — and its `targetType` checked against — the
/// right nodes: neither a non-doc nor a non-person can stand in.
#[tokio::test]
async fn cross_type_write_through_in_name_rejects_wrong_types() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_cross_type_schemas(&svc).await?;
    make_node(
        &svc,
        "4f1f600c-d552-57c4-b1b1-1f4676f32c07",
        "in_norm_person",
    )
    .await?;

    // p2 is not a doc, so `approved_by` is not declared on its type at all.
    let err = svc
        .create_relationship(
            "4f1f600c-d552-57c4-b1b1-1f4676f32c07",
            "approved_by",
            "99e6a416-b162-589d-97e9-af6fa8004f05",
            json!({}),
        )
        .await
        .expect_err("a person has no `approved_by`");
    assert!(
        err.to_string()
            .contains("'approved_by' not defined in schema 'in_norm_person'"),
        "got: {err}"
    );
    // doc1 -> doc1: the far end is a doc, which does not declare `approves`.
    let err = svc
        .create_relationship(
            "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167",
            "approved_by",
            "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167",
            json!({}),
        )
        .await
        .expect_err("a doc cannot approve");
    assert!(
        err.to_string()
            .contains("inbound view of 'in_norm_doc.approves'"),
        "got: {err}"
    );
    Ok(())
}

/// Reading through the `in` name narrows to the declaration's `targetType`:
/// a memo's `approves` edge is the same stored name but not an approval by a
/// person.
#[tokio::test]
async fn cross_type_read_through_in_name_narrows_to_declared_type() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_cross_type_schemas(&svc).await?;

    svc.create_relationship(
        "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167",
        "approved_by",
        "99e6a416-b162-589d-97e9-af6fa8004f05",
        json!({}),
    )
    .await?;
    svc.create_relationship(
        "d7ac97fb-f0b6-5af0-bc46-80281a91c768",
        "approves",
        "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167",
        json!({}),
    )
    .await?;

    let out = rel_ops::get_related_nodes(
        &svc,
        GetRelatedInput {
            node_id: "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167".to_string(),
            relationship_name: "approved_by".to_string(),
            direction: "out".to_string(),
        },
    )
    .await?;
    assert_eq!(out.count, 1);
    assert_eq!(
        out.related_nodes[0]["id"],
        "99e6a416-b162-589d-97e9-af6fa8004f05"
    );
    Ok(())
}

/// A `direction: in` declaration as `add_relationships` on `in_norm_doc`,
/// mirroring `in_norm_person.approves` except where `overrides` says.
fn doc_in_declaration(overrides: serde_json::Value) -> serde_json::Value {
    let mut rel = json!({
        "name": "endorsed_by",
        "targetType": "in_norm_person",
        "direction": "in",
        "cardinality": "many",
        "reverseName": "approves",
        "reverseCardinality": "many"
    });
    for (k, v) in overrides.as_object().cloned().unwrap_or_default() {
        rel[k] = v;
    }
    json!({ "schema_id": "in_norm_doc", "add_relationships": [rel] })
}

/// Saving an `in` declaration is rejected unless it exactly mirrors a forward
/// declaration on its `targetType` — otherwise it could never be written, or
/// would describe cardinality storage does not enforce.
#[tokio::test]
async fn unpaired_or_mismatched_in_declaration_is_rejected_at_save() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_cross_type_schemas(&svc).await?;

    // A "forward" that is itself an `in` declaration is not a forward: the
    // doc's `approved_by` is inbound, so nothing can mirror it.
    let err = handle_create_schema(
        &svc,
        json!({
            "name": "in_norm_team",
            "fields": [],
            "relationships": [{
                "name": "approval_of",
                "targetType": "in_norm_doc",
                "direction": "in",
                "cardinality": "many",
                "reverseName": "approved_by",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .expect_err("an in declaration mirroring another in declaration is rejected");
    assert!(
        err.to_string()
            .contains("does not declare 'approved_by' as \"direction\":\"out\""),
        "got: {err}"
    );

    // Drop the doc's valid declaration so the cases below can reuse its name
    // without colliding.
    handle_update_schema(
        &svc,
        json!({ "schema_id": "in_norm_doc", "remove_relationships": ["approved_by"] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("drop approved_by: {e}"))?;

    let cases = [
        // No forward `reviews` on the person at all.
        (
            json!({ "reverseName": "reviews" }),
            "does not declare 'reviews'",
        ),
        // The forward names a different reverse (`approved_by`).
        (json!({}), "its reverseName is 'approved_by'"),
        // Cardinality disagrees with the forward's reverseCardinality.
        (
            json!({ "name": "approved_by", "cardinality": "one" }),
            "its reverseCardinality (Many) differs",
        ),
        // reverseCardinality disagrees with the forward's cardinality.
        (
            json!({ "name": "approved_by", "reverseCardinality": "one" }),
            "its cardinality (Many) differs",
        ),
        // Edge attributes belong on the forward declaration.
        (
            json!({
                "name": "approved_by",
                "edgeFields": [{ "name": "note", "type": "string" }]
            }),
            "edgeFields",
        ),
        // No targetType to find the forward declaration on.
        (json!({ "targetType": null }), "without a targetType"),
    ];
    for (overrides, expected) in cases {
        let err = handle_update_schema(&svc, doc_in_declaration(overrides.clone()))
            .await
            .expect_err(&format!("{overrides} must be rejected"));
        assert!(
            err.to_string().contains(expected),
            "{overrides}: expected '{expected}' in: {err}"
        );
    }
    Ok(())
}

/// The self-referential pair is declared in one payload, before the schema
/// exists to be looked up, and must still validate.
#[tokio::test]
async fn self_referential_pair_in_one_payload_is_accepted() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_schema(&svc).await
}

/// Editing the forward side is validated against the `in` declarations that
/// mirror it: removing it, or re-adding it with a different reverse name,
/// would leave the doc's saved `approved_by` describing an edge storage no
/// longer has.
#[tokio::test]
async fn editing_forward_mirrored_by_in_declaration_is_rejected() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_cross_type_schemas(&svc).await?;

    let removed = handle_update_schema(
        &svc,
        json!({ "schema_id": "in_norm_person", "remove_relationships": ["approves"] }),
    )
    .await
    .expect_err("removing a mirrored forward must be rejected");
    assert!(
        removed
            .to_string()
            .contains("would break 'in_norm_doc.approved_by'"),
        "got: {removed}"
    );

    let redeclared = handle_update_schema(
        &svc,
        json!({
            "schema_id": "in_norm_person",
            "remove_relationships": ["approves"],
            "add_relationships": [{
                "name": "approves",
                "targetType": "in_norm_doc",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "endorsed_by",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .expect_err("re-adding the forward with another reverse name must be rejected");
    assert!(
        redeclared
            .to_string()
            .contains("its reverseName is 'endorsed_by'"),
        "got: {redeclared}"
    );
    Ok(())
}

/// A self-referential `in` declaration on a subtype mirrors a forward the
/// subtype inherits (ADR-078) — the same chain the write path resolves by.
#[tokio::test]
async fn in_declaration_mirroring_inherited_forward_is_accepted() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    handle_create_schema(
        &svc,
        json!({
            "name": "in_norm_base",
            "fields": [],
            "relationships": [{
                "name": "supersedes",
                "targetType": "in_norm_base",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "superseded_by",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("base schema: {e}"))?;
    handle_create_schema(
        &svc,
        json!({
            "name": "in_norm_sub",
            "extends": "in_norm_base",
            "fields": [],
            "relationships": [{
                "name": "superseded_by",
                "targetType": "in_norm_sub",
                "direction": "in",
                "cardinality": "one",
                "reverseName": "supersedes",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("sub schema: {e}"))?;

    make_node(&svc, "ac012a23-2b8e-5fdc-9aca-df129fe90625", "in_norm_sub").await?;
    make_node(&svc, "15881b3b-b2c4-5713-8dc5-03428d288eb7", "in_norm_sub").await?;
    svc.create_relationship(
        "ac012a23-2b8e-5fdc-9aca-df129fe90625",
        "superseded_by",
        "15881b3b-b2c4-5713-8dc5-03428d288eb7",
        json!({}),
    )
    .await?;
    assert_eq!(
        related_ids(
            &svc,
            "15881b3b-b2c4-5713-8dc5-03428d288eb7",
            "supersedes",
            "out"
        )
        .await?,
        ["ac012a23-2b8e-5fdc-9aca-df129fe90625"]
    );
    Ok(())
}

/// A call that both re-targets `extends` and adds an `in` declaration is
/// judged by its result: the forward lives on the NEW parent.
#[tokio::test]
async fn pairing_is_validated_after_same_call_extends_retarget() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    handle_create_schema(&svc, json!({ "name": "in_norm_parent_a", "fields": [] }))
        .await
        .map_err(|e| anyhow::anyhow!("parent a: {e}"))?;
    handle_create_schema(
        &svc,
        json!({
            "name": "in_norm_parent_b",
            "fields": [],
            "relationships": [{
                "name": "reviews",
                "targetType": "in_norm_parent_b",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "reviewed_by",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("parent b: {e}"))?;
    handle_create_schema(
        &svc,
        json!({ "name": "in_norm_child", "extends": "in_norm_parent_a", "fields": [] }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("child: {e}"))?;

    handle_update_schema(
        &svc,
        json!({
            "schema_id": "in_norm_child",
            "extends": "in_norm_parent_b",
            "add_relationships": [{
                "name": "reviewed_by",
                "targetType": "in_norm_child",
                "direction": "in",
                "cardinality": "many",
                "reverseName": "reviews",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("retarget + add in: {e}"))?;
    Ok(())
}

/// Re-parenting a MIDDLE schema can drop a forward a grandchild inherits. The
/// grandchild's stored chain still runs through the old parent, so validation
/// must splice in the chain the call leaves it with.
#[tokio::test]
async fn reparenting_middle_schema_that_drops_mirrored_forward_is_rejected() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    let create = |payload: serde_json::Value| {
        let svc = svc.clone();
        async move {
            handle_create_schema(&svc, payload)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        }
    };
    create(json!({ "name": "in_norm_target", "fields": [] })).await?;
    create(json!({
        "name": "in_norm_grand",
        "fields": [],
        "relationships": [{
            "name": "reviews",
            "targetType": "in_norm_target",
            "direction": "out",
            "cardinality": "many",
            "reverseName": "reviewed_by",
            "reverseCardinality": "many"
        }]
    }))
    .await?;
    create(json!({ "name": "in_norm_other_grand", "fields": [] })).await?;
    create(json!({ "name": "in_norm_middle", "extends": "in_norm_grand", "fields": [] })).await?;
    create(json!({ "name": "in_norm_leaf", "extends": "in_norm_middle", "fields": [] })).await?;
    // The target names its reviewers by the leaf type, whose forward is
    // inherited two levels up.
    handle_update_schema(
        &svc,
        json!({
            "schema_id": "in_norm_target",
            "add_relationships": [{
                "name": "reviewed_by",
                "targetType": "in_norm_leaf",
                "direction": "in",
                "cardinality": "many",
                "reverseName": "reviews",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("target reviewed_by: {e}"))?;

    let err = handle_update_schema(
        &svc,
        json!({ "schema_id": "in_norm_middle", "extends": "in_norm_other_grand" }),
    )
    .await
    .expect_err("re-parenting away from the declaring ancestor must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("would break 'in_norm_target.reviewed_by'"),
        "got: {message}"
    );
    assert_eq!(
        message.matches("invalid params").count(),
        1,
        "the wrapped error must not repeat its kind prefix: {message}"
    );
    Ok(())
}

/// `set_schema_relationships` writes declarations below schema save's pairing
/// check, so the forward half can still disappear there. A write through the
/// `in` name then has no storage shape to normalize to; the error names what
/// the caller wrote rather than the rewritten call.
#[tokio::test]
async fn write_through_in_name_after_forward_dropped_below_save_is_rejected() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_cross_type_schemas(&svc).await?;
    svc.set_schema_relationships("in_norm_person", &[]).await?;

    let err = svc
        .create_relationship(
            "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167",
            "approved_by",
            "99e6a416-b162-589d-97e9-af6fa8004f05",
            json!({}),
        )
        .await
        .expect_err("in_norm_person no longer declares `approves`");
    let message = err.to_string();
    assert!(
        message.contains("'approved_by' on 'in_norm_doc'")
            && message.contains("in_norm_person.approves"),
        "error should name the caller's spelling: {message}"
    );
    Ok(())
}

/// The relationship panel renders the edge once, as the forward declaration's
/// inbound group on the doc; an `in` declaration is never its own group on
/// either end.
#[tokio::test]
async fn panel_renders_in_declaration_edge_once() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_cross_type_schemas(&svc).await?;
    svc.create_relationship(
        "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167",
        "approved_by",
        "99e6a416-b162-589d-97e9-af6fa8004f05",
        json!({}),
    )
    .await?;

    let doc = rel_ops::get_node_relationships(&svc, "ee5ab1a7-1de8-531f-a862-9e9c1b2a2167").await?;
    assert!(
        !doc.groups
            .iter()
            .any(|g| g.relationship_name == "approved_by"),
        "own `in` declaration must not render as its own group"
    );
    let approvals = doc
        .groups
        .iter()
        .find(|g| {
            g.relationship_name == "approves"
                && g.direction == "in"
                && g.source_type == "in_norm_person"
        })
        .expect("person.approves inbound group");
    let ids: Vec<_> = approvals.related.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["99e6a416-b162-589d-97e9-af6fa8004f05"]);

    let person =
        rel_ops::get_node_relationships(&svc, "99e6a416-b162-589d-97e9-af6fa8004f05").await?;
    assert!(
        !person
            .groups
            .iter()
            .any(|g| g.relationship_name == "approved_by"),
        "another schema's `in` declaration is not an inbound declaration"
    );
    Ok(())
}
