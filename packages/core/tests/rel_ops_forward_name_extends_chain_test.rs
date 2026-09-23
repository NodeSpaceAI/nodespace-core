//! `resolve_relationship_name`'s forward-name check, against a subtype
//! reached only through the ADR-078 `extends` chain.
//!
//! `resolve_relationship_name`'s forward-name lookup used to consult
//! `node_type`'s own schema directly (`get_schema_node`), never the
//! `extends`-chain-merged set `NodeService::resolve_relationships` provides.
//! A relationship declared `Forward` only on an ancestor schema and inherited
//! (not redeclared) by a subtype was invisible to it -- resolution fell
//! through to the reverse-name checks, found nothing there either, and
//! returned `OpsError::InvalidParams`, the "undeclared name" error.
//!
//! The write path (`create_relationship`, via `resolve_declared_relationship`)
//! was already `extends`-chain aware, so a subtype instance could carry a
//! real edge that this read-side resolver refused to recognize as declared in
//! either direction at all -- distinct from (and deeper than) a cardinality
//! misclassification: the fetch itself never found the row. Per
//! `resolve_relationship_name`'s own doc comment this resolution is shared
//! with the CLI's read path and with `GraphResolver::fetch_related_nodes`
//! (which treats `InvalidParams` here as "undeclared, not a failure" and
//! silently returns an empty result), so the bug reached both.
//!
//! These tests cover the fix directly at the `rel_ops` layer: the forward
//! name resolves as `Forward` (not an error) for a subtype instance, and a
//! real edge attached to that instance is actually readable back through
//! `get_related_nodes`, not just correctly classified.

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::Node,
    ops::rel_ops::{self, ResolvedRelName},
    schema::handle_create_schema,
    services::NodeService,
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

/// Base type declares `story` forward, out to `rel_ops_ext_target`; the
/// subtype extends the base without redeclaring it.
async fn create_base_and_subtype(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "rel_ops_ext_target",
            "fields": [{ "name": "title", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("target schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "rel_ops_ext_base",
            "fields": [],
            "relationships": [{
                "name": "story",
                "targetType": "rel_ops_ext_target",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "tasks",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("base schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "rel_ops_ext_sub",
            "extends": "rel_ops_ext_base",
            "fields": []
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("subtype schema: {e}"))?;
    Ok(())
}

fn get(node_id: &str, name: &str, direction: &str) -> rel_ops::GetRelatedInput {
    rel_ops::GetRelatedInput {
        node_id: node_id.to_string(),
        relationship_name: name.to_string(),
        direction: direction.to_string(),
    }
}

/// Classification alone: a forward name declared only on the ancestor must
/// resolve as `Forward` for a subtype instance, not fall through to
/// `InvalidParams`.
#[tokio::test]
async fn inherited_forward_name_resolves_as_forward_not_invalid_params() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;
    make_node(&svc, "sub1", "rel_ops_ext_sub").await?;

    let resolved =
        rel_ops::resolve_relationship_name(&svc, "sub1", "rel_ops_ext_sub", "story").await?;
    assert_eq!(resolved, ResolvedRelName::Forward);
    Ok(())
}

/// The actual bug: classification alone isn't enough -- a real edge attached
/// to a subtype instance (via the already chain-aware write path) must be
/// readable back through `get_related_nodes`, not just resolve without
/// erroring. Before the fix this returned `Err(InvalidParams)` here, which
/// `GraphResolver::fetch_related_nodes` turns into a silent empty result.
#[tokio::test]
async fn real_edge_on_subtype_instance_is_readable_through_inherited_forward_name() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;
    make_node(&svc, "target1", "rel_ops_ext_target").await?;
    make_node(&svc, "sub1", "rel_ops_ext_sub").await?;

    // The write path is already extends-chain aware -- this succeeds today
    // even though `story` is declared only on the ancestor schema.
    svc.create_relationship("sub1", "story", "target1", json!({}))
        .await
        .expect("create_relationship must succeed for an inherited relationship");

    let out = rel_ops::get_related_nodes(&svc, get("sub1", "story", "out")).await?;
    assert_eq!(
        out.count, 1,
        "a real edge on a subtype instance must be readable through an \
         inherited forward name, not silently empty"
    );
    assert_eq!(out.related_nodes[0]["id"], "target1");
    assert_eq!(out.relationship_name, "story");
    assert_eq!(out.direction, "out");
    Ok(())
}

/// An undeclared name on a subtype must still error -- the fix must not
/// make the forward check accept anything not actually in the chain-merged
/// relationship set.
#[tokio::test]
async fn undeclared_name_on_subtype_still_errors() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;
    make_node(&svc, "sub1", "rel_ops_ext_sub").await?;

    let err = rel_ops::get_related_nodes(&svc, get("sub1", "not_a_real_name", "out"))
        .await
        .expect_err("an undeclared name must still error, extends chain or not");
    assert!(matches!(
        err,
        nodespace_core::ops::OpsError::InvalidParams(_)
    ));
    Ok(())
}

/// The type-system `extends` relationship itself is excluded from the
/// chain-merged set `resolve_relationships` returns (it is a statement about
/// the schema graph, not a real per-instance relationship -- see that
/// function's own doc comment). Resolving the literal name `"extends"` on a
/// subtype instance must not turn into a hard error just because the fix
/// stopped the forward check from matching it directly: `extends` is itself
/// stored as an ordinary declaration row whose `target_type` is the parent
/// (an ancestor of the subtype's own type chain), so `get_inbound_relationships`
/// independently picks it up as an inbound-forward name on the subtype it was
/// declared on -- a path this fix does not touch. `"extends"` therefore keeps
/// resolving (as `InboundForward`, not `Forward`) and stays a real, empty
/// (not erroring) traversal, exactly as before the fix.
#[tokio::test]
async fn extends_literal_name_still_resolves_without_erroring() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;
    make_node(&svc, "sub1", "rel_ops_ext_sub").await?;

    let resolved =
        rel_ops::resolve_relationship_name(&svc, "sub1", "rel_ops_ext_sub", "extends").await?;
    assert_eq!(
        resolved,
        ResolvedRelName::InboundForward,
        "the type-system 'extends' name must keep resolving, not error, even \
         though it is no longer matched by the (now chain-aware) forward check"
    );

    let out = rel_ops::get_related_nodes(&svc, get("sub1", "extends", "out")).await?;
    assert_eq!(
        out.count, 0,
        "no data node ever carries a real 'extends' edge -- this must stay a \
         declared-shaped empty result, not an error"
    );
    Ok(())
}

/// The documented precedence rule ("a forward name always wins over a
/// same-spelled reverse name on another schema") already applied to a
/// subtype's OWN directly-declared relationships before this fix. Making the
/// forward check chain-aware necessarily extends that same rule across the
/// extends chain: a forward name declared only on an ancestor and inherited
/// by a subtype must still win over an unrelated schema's same-spelled
/// `reverseName`, for exactly the same reason it already won when declared
/// directly -- picking the reverse match instead would mean the real,
/// chain-aware edge the write path attached under the forward name (`shared`,
/// out to `rel_ops_ext_target`) becomes invisible again, silently replaced by
/// a same-named but unrelated traversal.
#[tokio::test]
async fn inherited_forward_name_still_wins_over_another_schemas_same_spelled_reverse_name(
) -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;

    // A third, unrelated schema declares a DIFFERENT forward relationship
    // whose reverseName collides with the base/subtype's own forward name
    // ("story"), targeting the subtype.
    handle_create_schema(
        &svc,
        json!({
            "name": "rel_ops_ext_other",
            "fields": [],
            "relationships": [{
                "name": "other_forward",
                "targetType": "rel_ops_ext_sub",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "story",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("other schema: {e}"))?;

    make_node(&svc, "target1", "rel_ops_ext_target").await?;
    make_node(&svc, "sub1", "rel_ops_ext_sub").await?;
    make_node(&svc, "other1", "rel_ops_ext_other").await?;

    // The real, chain-aware edge under the inherited forward name.
    svc.create_relationship("sub1", "story", "target1", json!({}))
        .await
        .expect("create_relationship must succeed for an inherited relationship");
    // The colliding edge under the OTHER schema's forward declaration.
    svc.create_relationship("other1", "other_forward", "sub1", json!({}))
        .await
        .expect("create_relationship must succeed for the colliding declaration");

    let resolved =
        rel_ops::resolve_relationship_name(&svc, "sub1", "rel_ops_ext_sub", "story").await?;
    assert_eq!(
        resolved,
        ResolvedRelName::Forward,
        "the inherited forward name must win over the same-spelled reverse \
         name declared elsewhere, exactly as an own-declared forward name would"
    );

    let out = rel_ops::get_related_nodes(&svc, get("sub1", "story", "out")).await?;
    assert_eq!(
        out.count, 1,
        "must return the real edge under the inherited forward name, not the \
         colliding schema's edge"
    );
    assert_eq!(out.related_nodes[0]["id"], "target1");
    Ok(())
}

/// The relationship-viewer aggregation (`get_node_relationships`) has the
/// identical own-schema-only pattern as `resolve_relationship_name` did:
/// its outbound-group loop used to consult `get_schema_node(&node_type)`
/// directly. A relationship declared only on an ancestor and inherited by a
/// subtype rendered no outbound group at all -- not an empty one, just
/// absent -- even though the write path already accepted edges under it.
#[tokio::test]
async fn viewer_shows_an_outbound_group_for_an_inherited_relationship() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_base_and_subtype(&svc).await?;
    make_node(&svc, "sub1", "rel_ops_ext_sub").await?;

    let viewer = rel_ops::get_node_relationships(&svc, "sub1").await?;
    let group = viewer
        .groups
        .iter()
        .find(|g| g.relationship_name == "story" && g.direction == "out")
        .expect(
            "an inherited outbound relationship must still render as a group, \
             even with zero edges yet",
        );
    assert_eq!(group.target_type.as_deref(), Some("rel_ops_ext_target"));
    assert_eq!(group.count, 0);

    // And once a real edge exists, the group reflects it.
    make_node(&svc, "target1", "rel_ops_ext_target").await?;
    svc.create_relationship("sub1", "story", "target1", json!({}))
        .await
        .expect("create_relationship must succeed for an inherited relationship");
    let viewer = rel_ops::get_node_relationships(&svc, "sub1").await?;
    let group = viewer
        .groups
        .iter()
        .find(|g| g.relationship_name == "story" && g.direction == "out")
        .expect("outbound group");
    assert_eq!(group.count, 1);
    assert_eq!(group.related[0].id, "target1");
    Ok(())
}

/// `get_related_nodes`'s Reverse-narrowing filter matched the declaring
/// schema's exact `node_type`, not its whole descendant set (ADR-078) -- the
/// same failure mode `graph_resolver.rs`'s reverse-segment walk already
/// guards against, for the identical reason. `blocks` is declared only on
/// `rel_ops_ext_reltask` (self-referential); `rel_ops_ext_relissue` extends
/// it without redeclaring. An issue blocking another issue is a real edge
/// under the inherited forward name (already correctly attached, per the
/// forward-name fix above) -- but reading it back via the reverse name
/// `blocked_by` used to compare the blocking node's concrete type
/// (`rel_ops_ext_relissue`) against the declaring type
/// (`rel_ops_ext_reltask`) with exact equality, silently dropping it: "issue
/// blocks issue" read as "nothing blocks this."
#[tokio::test]
async fn reverse_name_narrowing_includes_a_subtype_of_the_declaring_schema() -> Result<()> {
    let (svc, _t) = create_test_service().await?;

    handle_create_schema(
        &svc,
        json!({
            "name": "rel_ops_ext_reltask",
            "fields": [],
            "relationships": [{
                "name": "blocks",
                "targetType": "rel_ops_ext_reltask",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "blocked_by",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("task schema: {e}"))?;

    handle_create_schema(
        &svc,
        json!({
            "name": "rel_ops_ext_relissue",
            "extends": "rel_ops_ext_reltask",
            "fields": []
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("issue schema: {e}"))?;

    make_node(&svc, "issue1", "rel_ops_ext_relissue").await?;
    make_node(&svc, "issue2", "rel_ops_ext_relissue").await?;
    svc.create_relationship("issue1", "blocks", "issue2", json!({}))
        .await
        .expect("create_relationship must succeed for an inherited relationship");

    let resolved =
        rel_ops::resolve_relationship_name(&svc, "issue2", "rel_ops_ext_relissue", "blocked_by")
            .await?;
    assert_eq!(
        resolved,
        ResolvedRelName::Reverse {
            forward_name: "blocks".to_string(),
            source_type: Some("rel_ops_ext_reltask".to_string()),
        }
    );

    let out = rel_ops::get_related_nodes(&svc, get("issue2", "blocked_by", "out")).await?;
    assert_eq!(
        out.count, 1,
        "a subtype instance's blocking edge must survive the reverse-name \
         narrowing filter, not just an exact-type match"
    );
    assert_eq!(out.related_nodes[0]["id"], "issue1");
    Ok(())
}
