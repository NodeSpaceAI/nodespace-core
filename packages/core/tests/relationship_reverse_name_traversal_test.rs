//! Traversal by a schema-declared `reverseName`.
//!
//! A relationship declares a forward `name` and, optionally, a `reverseName` —
//! the name for the same edge read from the target's end (`adr.decided_by →
//! reviewer` with `reverseName: "decisions"`). Edges are stored only under the
//! forward name, so a reverse name reached the store as a literal that matched
//! no row and came back as an empty result: silently indistinguishable from
//! "this person has decided nothing", for the query that is the whole reason to
//! model with relationships rather than a text field.
//!
//! `rel_ops::get_related_nodes` now resolves the supplied name against the
//! node's schema neighbourhood before traversing. These tests cover:
//!
//! - a reverse name returns exactly what the forward name with the opposite
//!   direction returns, and reports the traversal it actually ran
//! - a forward name is untouched, including one that collides with another
//!   type's reverse name
//! - an undeclared name errors instead of returning a silent zero, while a
//!   declared name with no edges still returns an ordinary empty result
//! - built-in structural names still traverse without being declared
//! - a reverse name combined with `--direction in` ignores the requested
//!   direction and still returns the real traversal, rather than resolving
//!   the name and then flipping direction a second time onto a query that
//!   can never have edges (the double-reversal that silently returned zero)

use anyhow::Result;
use nodespace_core::{
    db::SqliteStore,
    models::Node,
    ops::{rel_ops, OpsError},
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

/// The issue's own shape: `adr.decided_by → reviewer`, reversed as `decisions`.
async fn create_adr_pair(svc: &Arc<NodeService>) -> Result<()> {
    handle_create_schema(
        svc,
        json!({
            "name": "Reviewer",
            "fields": [{ "name": "email", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("reviewer schema: {e}"))?;

    handle_create_schema(
        svc,
        json!({
            "name": "Adr",
            "fields": [{ "name": "status", "type": "string", "protection": "user", "indexed": false }],
            "relationships": [{
                "name": "decided_by",
                "targetType": "reviewer",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "decisions",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("adr schema: {e}"))?;
    Ok(())
}

fn get(node_id: &str, name: &str, direction: &str) -> rel_ops::GetRelatedInput {
    rel_ops::GetRelatedInput {
        node_id: node_id.to_string(),
        relationship_name: name.to_string(),
        direction: direction.to_string(),
    }
}

/// The reported defect: the declared reverse name returned 0 silently, while
/// the same traversal spelled `--type decided_by --direction in` returned the
/// ADR. Both spellings must now agree.
#[tokio::test]
async fn reverse_name_matches_forward_name_with_direction_in() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_pair(&svc).await?;
    make_node(&svc, "p1", "reviewer").await?;
    make_node(&svc, "adr1", "adr").await?;
    svc.create_relationship("adr1", "decided_by", "p1", json!({}))
        .await?;

    let by_reverse = rel_ops::get_related_nodes(&svc, get("p1", "decisions", "out")).await?;
    let by_forward = rel_ops::get_related_nodes(&svc, get("p1", "decided_by", "in")).await?;

    assert_eq!(by_reverse.count, 1, "reverse name must not return a zero");
    assert_eq!(by_reverse.count, by_forward.count);
    assert_eq!(by_reverse.related_nodes, by_forward.related_nodes);

    // The response describes the traversal that actually ran, so the CLI's
    // `[<id> --<name>--> ]` line reports an inbound edge rather than drawing an
    // outbound arrow for a name that resolved the other way.
    assert_eq!(by_reverse.relationship_name, "decided_by");
    assert_eq!(by_reverse.direction, "in");
    Ok(())
}

/// A reverse name names one specific traversal — there is no second direction
/// it could also mean — so `--direction` is ignored once a name resolves as
/// reverse. `decisions --direction in` must return exactly what `decisions`
/// (default `out`) returns, not the forward name's true outbound side (which
/// is empty here and, applied through the reverse alias, was indistinguishable
/// from "no edges exist" — the double-reversal this test used to enshrine).
#[tokio::test]
async fn reverse_name_ignores_the_requested_direction() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_pair(&svc).await?;
    make_node(&svc, "p1", "reviewer").await?;
    make_node(&svc, "adr1", "adr").await?;
    svc.create_relationship("adr1", "decided_by", "p1", json!({}))
        .await?;

    let via_in = rel_ops::get_related_nodes(&svc, get("p1", "decisions", "in")).await?;
    let via_out = rel_ops::get_related_nodes(&svc, get("p1", "decisions", "out")).await?;
    assert_eq!(via_in.relationship_name, "decided_by");
    assert_eq!(via_in.direction, "in");
    assert_eq!(
        via_in.count, 1,
        "reverse name + --direction in must not double-reverse to zero"
    );
    assert_eq!(via_in.related_nodes, via_out.related_nodes);
    Ok(())
}

/// Resolution must never redirect a traversal that already worked: the forward
/// name is matched on the node's own schema first and passes through verbatim.
#[tokio::test]
async fn forward_name_is_unchanged_by_resolution() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_pair(&svc).await?;
    make_node(&svc, "p1", "reviewer").await?;
    make_node(&svc, "adr1", "adr").await?;
    svc.create_relationship("adr1", "decided_by", "p1", json!({}))
        .await?;

    let out = rel_ops::get_related_nodes(&svc, get("adr1", "decided_by", "out")).await?;
    assert_eq!(out.relationship_name, "decided_by");
    assert_eq!(out.direction, "out");
    assert_eq!(out.count, 1);
    Ok(())
}

/// The workaround the bug forced callers onto — the forward name of an inbound
/// relationship, read with `--direction in` — worked before resolution existed
/// and must keep working. The name is not declared on the *querying* node's own
/// schema, so a resolver that only consulted that schema would break it.
#[tokio::test]
async fn inbound_forward_name_still_traverses() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_pair(&svc).await?;
    make_node(&svc, "p1", "reviewer").await?;
    make_node(&svc, "adr1", "adr").await?;
    svc.create_relationship("adr1", "decided_by", "p1", json!({}))
        .await?;

    let inbound = rel_ops::get_related_nodes(&svc, get("p1", "decided_by", "in")).await?;
    assert_eq!(inbound.relationship_name, "decided_by");
    assert_eq!(inbound.direction, "in");
    assert_eq!(inbound.count, 1);
    assert_eq!(inbound.related_nodes[0]["id"], "adr1");
    Ok(())
}

/// A declared name with no edges is a legitimate empty answer and must stay
/// one — the fix distinguishes "declared, nothing linked" from "not a name at
/// all", it does not make emptiness itself an error.
#[tokio::test]
async fn declared_name_with_no_edges_returns_empty_not_an_error() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_pair(&svc).await?;
    make_node(&svc, "p1", "reviewer").await?;

    let by_reverse = rel_ops::get_related_nodes(&svc, get("p1", "decisions", "out")).await?;
    assert_eq!(by_reverse.count, 0);
    assert!(by_reverse.related_nodes.is_empty());

    make_node(&svc, "adr1", "adr").await?;
    let forward = rel_ops::get_related_nodes(&svc, get("adr1", "decided_by", "out")).await?;
    assert_eq!(forward.count, 0);
    Ok(())
}

/// The harmful case the issue names: an undeclared name must be distinguishable
/// from a declared one with no edges. It errors, and the error names both the
/// working spellings so the call can be repaired without guessing.
#[tokio::test]
async fn undeclared_name_errors_instead_of_returning_zero() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    create_adr_pair(&svc).await?;
    make_node(&svc, "p1", "reviewer").await?;

    let err = rel_ops::get_related_nodes(&svc, get("p1", "desicions", "out"))
        .await
        .expect_err("a name declared in neither direction must not report as zero results");

    match err {
        OpsError::InvalidParams(msg) => {
            assert!(
                msg.contains("desicions"),
                "error names the bad input: {msg}"
            );
            assert!(msg.contains("reviewer"), "error names the node type: {msg}");
            assert!(
                msg.contains("decisions"),
                "error names the reverse spelling that works: {msg}"
            );
            // BOTH working spellings must be offered. A relationship carrying a
            // reverse_name is still reachable by its forward name read inbound,
            // and listing only the reverse one omits a spelling that
            // demonstrably works — the same under-reporting this error exists to
            // prevent. Asserted separately from the reverse name because a
            // regrouping of this list once dropped exactly this entry while the
            // reverse-name assertion above kept passing.
            assert!(
                msg.contains("decided_by"),
                "error names the forward spelling that also works: {msg}"
            );
        }
        other => panic!("expected InvalidParams, got {other:?}"),
    }

    // The forward spelling the error advertises must actually work.
    make_node(&svc, "adr1", "adr").await?;
    svc.create_relationship("adr1", "decided_by", "p1", json!({}))
        .await?;
    let advertised = rel_ops::get_related_nodes(&svc, get("p1", "decided_by", "in")).await?;
    assert_eq!(advertised.count, 1);
    Ok(())
}

/// A type with no schema at all (plain text) still gets a real error rather
/// than a zero, and the message says plainly that nothing is declared.
#[tokio::test]
async fn unschema_d_node_type_reports_no_declared_relationships() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    make_node(&svc, "t1", "text").await?;

    let err = rel_ops::get_related_nodes(&svc, get("t1", "decisions", "out"))
        .await
        .expect_err("a text node declares no typed relationships");

    match err {
        OpsError::InvalidParams(msg) => assert!(
            msg.contains("decisions") && msg.contains("text"),
            "unhelpful message: {msg}"
        ),
        other => panic!("expected InvalidParams, got {other:?}"),
    }
    Ok(())
}

/// Built-in structural names are legal between any two nodes without being
/// declared on a schema, so resolution must let them through untouched.
#[tokio::test]
async fn builtin_relationship_names_still_traverse() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    make_node(&svc, "t1", "text").await?;
    make_node(&svc, "t2", "text").await?;
    svc.create_relationship("t1", "mentions", "t2", json!({}))
        .await?;

    let out = rel_ops::get_related_nodes(&svc, get("t1", "mentions", "out")).await?;
    assert_eq!(out.relationship_name, "mentions");
    assert_eq!(out.direction, "out");
    assert_eq!(out.count, 1);
    Ok(())
}

/// An untyped relationship (`targetType` omitted — the documented escape hatch
/// for "the target type doesn't exist yet") matches EVERY type inbound, so its
/// reverse name would otherwise resolve from any node in the workspace and
/// return a guaranteed zero: the silent zero this fix exists to eliminate,
/// coming back through a side door. It must be reachable from a type edges
/// actually connect, and rejected from one they never do.
#[tokio::test]
async fn untyped_reverse_name_resolves_only_where_edges_reach() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    handle_create_schema(
        &svc,
        json!({
            "name": "Tag",
            "fields": [{ "name": "label", "type": "string", "protection": "user", "indexed": false }],
            // No targetType: this relationship may point at anything.
            "relationships": [{
                "name": "tagged_with",
                "direction": "out",
                "cardinality": "many",
                "reverseName": "tagged_items",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("tag schema: {e}"))?;

    make_node(&svc, "tag1", "tag").await?;
    make_node(&svc, "t1", "text").await?;
    svc.create_relationship("tag1", "tagged_with", "t1", json!({}))
        .await?;

    // A text node an untyped edge actually reaches: the reverse name answers.
    let reached = rel_ops::get_related_nodes(&svc, get("t1", "tagged_items", "out")).await?;
    assert_eq!(reached.relationship_name, "tagged_with");
    assert_eq!(reached.direction, "in");
    assert_eq!(reached.count, 1);
    assert_eq!(reached.related_nodes[0]["id"], "tag1");

    // A type no untyped edge reaches must NOT resolve to a guaranteed zero.
    make_node(&svc, "2026-01-15", "date").await?;
    let err = rel_ops::get_related_nodes(&svc, get("2026-01-15", "tagged_items", "out"))
        .await
        .expect_err("an untyped reverse name must not silently return 0 from an unrelated type");
    match err {
        OpsError::InvalidParams(msg) => assert!(
            msg.contains("tagged_items"),
            "error should name the attempted relationship: {msg}"
        ),
        other => panic!("expected InvalidParams, got {other:?}"),
    }

    // And it must not pollute an unrelated type's "available" list.
    let err = rel_ops::get_related_nodes(&svc, get("2026-01-15", "zzz_bogus", "out"))
        .await
        .expect_err("undeclared name still errors");
    match err {
        OpsError::InvalidParams(msg) => assert!(
            !msg.contains("tagged_items"),
            "an untyped relationship that reaches nothing here must not be offered: {msg}"
        ),
        other => panic!("expected InvalidParams, got {other:?}"),
    }
    Ok(())
}

/// Two schemas may declare the same forward name toward one type. The store's
/// "in" query keys on the name alone, so each schema's reverse name must be
/// narrowed to its own declaring type rather than sweeping in the other
/// schema's edges and over-reporting the count.
#[tokio::test]
async fn reverse_name_is_scoped_to_its_declaring_type() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    handle_create_schema(
        &svc,
        json!({
            "name": "Reviewer",
            "fields": [{ "name": "email", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("reviewer schema: {e}"))?;

    // Both `adr` and `memo` declare `decided_by → reviewer`, each naming its own
    // reverse. Since every declaration must name the edge from both ends, the
    // collision this test guards is now between two live reverse names sharing
    // one forward name — not between a named and an unnamed one.
    for (name, reverse) in [("Adr", "decisions"), ("Memo", "memos")] {
        let rel = json!({
            "name": "decided_by",
            "targetType": "reviewer",
            "direction": "out",
            "cardinality": "one",
            "reverseName": reverse,
            "reverseCardinality": "many"
        });
        handle_create_schema(
            &svc,
            json!({
                "name": name,
                "fields": [{ "name": "status", "type": "string", "protection": "user", "indexed": false }],
                "relationships": [rel]
            }),
        )
        .await
        .map_err(|e| anyhow::anyhow!("{name} schema: {e}"))?;
    }

    make_node(&svc, "p1", "reviewer").await?;
    make_node(&svc, "adr1", "adr").await?;
    make_node(&svc, "memo1", "memo").await?;
    svc.create_relationship("adr1", "decided_by", "p1", json!({}))
        .await?;
    svc.create_relationship("memo1", "decided_by", "p1", json!({}))
        .await?;

    let by_reverse = rel_ops::get_related_nodes(&svc, get("p1", "decisions", "out")).await?;
    assert_eq!(
        by_reverse.count, 1,
        "`decisions` is adr's reverse name — it must not also collect memo's edges"
    );
    assert_eq!(
        by_reverse.related_nodes[0]["id"], "adr1",
        "wrong declaring type surfaced under the reverse name"
    );

    // The symmetric case: memo's own reverse name resolves only to memo's edge.
    let by_memo_reverse = rel_ops::get_related_nodes(&svc, get("p1", "memos", "out")).await?;
    assert_eq!(
        by_memo_reverse.count, 1,
        "`memos` is memo's reverse name — it must not also collect adr's edges"
    );
    assert_eq!(
        by_memo_reverse.related_nodes[0]["id"], "memo1",
        "wrong declaring type surfaced under the reverse name"
    );
    Ok(())
}

/// A self-referential relationship's `reverseName`.
///
/// Schema-authoring guidance recommends `reverseName` over a second
/// declaration for self-reference (`supersedes` on `adr` reading back as
/// `superseded_by`), because it is one stored edge readable from both ends.
/// That recommendation is only sound if resolution handles the case where the
/// declaring type and the target type are the SAME type — where the schema
/// appears in its own inbound set and the forward name is reachable from both
/// endpoints of the same edge.
#[tokio::test]
async fn self_referential_reverse_name_resolves() -> Result<()> {
    let (svc, _t) = create_test_service().await?;

    handle_create_schema(
        &svc,
        json!({
            "name": "Adr",
            "fields": [{ "name": "status", "type": "string", "protection": "user", "indexed": false }],
            "relationships": [{
                "name": "supersedes",
                "targetType": "adr",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "superseded_by",
                "reverseCardinality": "one"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("adr schema: {e}"))?;

    make_node(&svc, "adr_new", "adr").await?;
    make_node(&svc, "adr_old", "adr").await?;
    // The new ADR supersedes the old one.
    svc.create_relationship("adr_new", "supersedes", "adr_old", json!({}))
        .await?;

    // Forward, from the superseding end.
    let forward = rel_ops::get_related_nodes(&svc, get("adr_new", "supersedes", "out")).await?;
    assert_eq!(
        forward.count, 1,
        "forward traversal should find the old ADR"
    );
    assert_eq!(forward.related_nodes[0]["id"], "adr_old");

    // The reverse name, from the superseded end. This is the spelling the
    // guidance tells authors to use, and the one that returned a silent zero
    // before reverse-name resolution existed.
    let reverse = rel_ops::get_related_nodes(&svc, get("adr_old", "superseded_by", "out")).await?;
    assert_eq!(
        reverse.count, 1,
        "the declared reverseName must resolve on a self-referential relationship, \
         not return a silent zero: {reverse:?}"
    );
    assert_eq!(
        reverse.related_nodes[0]["id"], "adr_new",
        "superseded_by should surface the ADR that supersedes this one"
    );

    // And it must agree with the pre-existing spelling of the same traversal.
    let inbound_forward =
        rel_ops::get_related_nodes(&svc, get("adr_old", "supersedes", "in")).await?;
    assert_eq!(
        inbound_forward.related_nodes[0]["id"], reverse.related_nodes[0]["id"],
        "reverseName and `--type supersedes --direction in` must agree"
    );
    Ok(())
}

/// The exact regression reported: `Invoice.billed_to → Customer`, reverseName
/// `invoices`. `--type invoices --direction in` used to resolve `invoices` to
/// its forward form `billed_to`, then flip `--direction in` onto that
/// already-reversed name — landing on `(billed_to, out)`, which can never have
/// edges (a customer is never the source of a `billed_to` edge) and silently
/// returned `count: 0, exit 0`, indistinguishable from "no invoices billed to
/// this customer". All three spellings from the issue must now agree.
#[tokio::test]
async fn reverse_name_with_direction_in_matches_the_issue_repro() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    handle_create_schema(
        &svc,
        json!({
            "name": "Customer",
            "fields": [{ "name": "email", "type": "string", "protection": "user", "indexed": false }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("customer schema: {e}"))?;

    handle_create_schema(
        &svc,
        json!({
            "name": "Invoice",
            "fields": [{ "name": "amount", "type": "number", "protection": "user", "indexed": false }],
            "relationships": [{
                "name": "billed_to",
                "targetType": "customer",
                "direction": "out",
                "cardinality": "one",
                "reverseName": "invoices",
                "reverseCardinality": "many"
            }]
        }),
    )
    .await
    .map_err(|e| anyhow::anyhow!("invoice schema: {e}"))?;

    make_node(&svc, "cust1", "customer").await?;
    make_node(&svc, "inv1", "invoice").await?;
    make_node(&svc, "inv2", "invoice").await?;
    svc.create_relationship("inv1", "billed_to", "cust1", json!({}))
        .await?;
    svc.create_relationship("inv2", "billed_to", "cust1", json!({}))
        .await?;

    // `--type billed_to --direction in` — the pre-existing workaround.
    let by_forward_in = rel_ops::get_related_nodes(&svc, get("cust1", "billed_to", "in")).await?;
    assert_eq!(by_forward_in.count, 2);

    // `--type invoices` (default out) — the declared reverseName.
    let by_reverse_default =
        rel_ops::get_related_nodes(&svc, get("cust1", "invoices", "out")).await?;
    assert_eq!(by_reverse_default.count, 2);

    // `--type invoices --direction in` — the reported bug. Must not silently
    // return 0; must agree with the other two spellings.
    let by_reverse_in = rel_ops::get_related_nodes(&svc, get("cust1", "invoices", "in")).await?;
    assert_eq!(
        by_reverse_in.count, 2,
        "reverseName + --direction in must not silently double-reverse to zero"
    );
    assert_eq!(by_reverse_in.relationship_name, "billed_to");
    assert_eq!(by_reverse_in.direction, "in");

    let ids = |output: &rel_ops::GetRelatedOutput| -> Vec<String> {
        let mut ids: Vec<String> = output
            .related_nodes
            .iter()
            .map(|n| n["id"].as_str().unwrap().to_string())
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(ids(&by_reverse_in), ids(&by_forward_in));
    assert_eq!(ids(&by_reverse_in), ids(&by_reverse_default));
    Ok(())
}

/// The same double-reversal, for a built-in's fixed inverse spelling rather
/// than a schema-declared `reverseName`. `ResolvedRelName::Reverse` covers
/// both — a built-in reverse name is just as much "one specific traversal,
/// no second direction" as a declared one, so `child_of --direction in` must
/// resolve identically to `child_of` (default `out`), not flip onto
/// `has_child --direction out` (this node's own children — a different,
/// silently wrong answer for anything but a node with no children).
#[tokio::test]
async fn builtin_reverse_name_ignores_the_requested_direction() -> Result<()> {
    let (svc, _t) = create_test_service().await?;
    make_node(&svc, "parent1", "text").await?;
    make_node(&svc, "child1", "text").await?;
    svc.create_relationship("parent1", "has_child", "child1", json!({}))
        .await?;

    let via_default = rel_ops::get_related_nodes(&svc, get("child1", "child_of", "out")).await?;
    let via_in = rel_ops::get_related_nodes(&svc, get("child1", "child_of", "in")).await?;

    assert_eq!(via_default.count, 1);
    assert_eq!(via_default.related_nodes[0]["id"], "parent1");
    assert_eq!(
        via_in.count, 1,
        "built-in reverse name + --direction in must not double-reverse to zero"
    );
    assert_eq!(via_in.related_nodes, via_default.related_nodes);
    assert_eq!(via_in.relationship_name, "has_child");
    assert_eq!(via_in.direction, "in");
    Ok(())
}
