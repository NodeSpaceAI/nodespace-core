//! Winnability audit for the agent-matrix scenarios that fail for EVERY model.
//!
//! Scenarios 6, 8e and 11d fail across every model measured, including one that
//! passes 17/20 overall and clears six scenarios the local model fails. When a
//! model that capable still cannot pass them, the scenario is the suspect —
//! and the audit that follows has to answer a question the matrix itself
//! cannot: is the turn winnable at all?
//!
//! "Winnable" here has a precise, two-part meaning, and BOTH parts have to hold:
//!
//!   1. The facts the prompt depends on are present in what the model is given.
//!      A prompt that refers to something the model cannot see is unwinnable no
//!      matter how it is worded, and no amount of prose tuning fixes a missing
//!      fact.
//!   2. The tool call the assertion demands is ACCEPTED by a live backend
//!      holding the same seeded state. A well-formed ideal call that the
//!      validator rejects proves the scenario unwinnable without needing a
//!      model to demonstrate it — the shape that made an earlier draft of 11c
//!      unwinnable, where the documented generic relation name was refused.
//!
//! This file covers part 2 — the executor half — against a real SQLite-backed
//! `NodeService`. Part 1 is covered by the history-rendering tests in
//! `daemon/src/services/local_agent_service.rs`, because the facts a turn can
//! see are produced by `node_history_from_messages` and are checkable there
//! without a model or a daemon.
//!
//! WHAT THIS FILE IS NOT. It does not assert that any model DOES make these
//! calls — that is what the matrix measures, and it needs live inference. It
//! asserts only that a model which made the ideal call would be accepted. That
//! separation is the point: it splits "the model did not do it" from "the
//! system would not have let it", which a matrix run alone cannot distinguish.

use nodespace_agent::agent_types::AgentToolExecutor;
use nodespace_agent::local_agent::tools::GraphToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::services::NodeService;
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;

async fn make_executor() -> (GraphToolExecutor, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("winnability.db");
    let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
    let svc = Arc::new(NodeService::new(&mut store).await.unwrap());
    let executor = GraphToolExecutor {
        node_service: Some(svc),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, tmp)
}

/// Strip the `nodespace://` prefix a tool result reports ids under.
fn bare(id: &str) -> &str {
    id.strip_prefix("nodespace://").unwrap_or(id)
}

async fn call(executor: &GraphToolExecutor, tool: &str, args: Value) -> Value {
    let result = executor
        .execute(tool, args)
        .await
        .unwrap_or_else(|e| panic!("{tool} returned a transport error: {e:?}"));
    assert!(
        !result.is_error,
        "{tool} was REJECTED — the ideal call for this scenario cannot be made, \
         so the scenario is unwinnable by construction rather than by model \
         capability: {:?}",
        result.result
    );
    result.result
}

/// Seed 11a/11b/11c/11c2's graph state and return `(decision_id, task_id)`.
///
/// Uses the real tool path rather than `NodeService` directly, so the seeded
/// state is exactly what the scenario's own earlier turns would have produced —
/// including the `nodespace://` id spelling a later turn has to copy.
async fn seed_scenario_11(executor: &GraphToolExecutor) -> (String, String) {
    let decision = call(
        executor,
        "create_node",
        json!({
            "content": "the reports page uses server-side rendering",
            "node_type": "text",
        }),
    )
    .await;
    let task = call(
        executor,
        "create_node",
        json!({
            "content": "rebuild the reports page",
            "node_type": "task",
        }),
    )
    .await;

    let decision_id = bare(decision["id"].as_str().unwrap()).to_string();
    let task_id = bare(task["id"].as_str().unwrap()).to_string();

    // 11c's ideal call. `mentions` is one of the four universal relation names
    // legal between any two records; an ad-hoc name here would be refused
    // regardless of the endpoints, which is the trap 11c's own comment records.
    call(
        executor,
        "create_relationship",
        json!({
            "from_id": task_id,
            "to_id": decision_id,
            "relationship_type": "mentions",
        }),
    )
    .await;

    // 11c2's ideal calls: a second source node, linked to the SAME decision.
    // Two inbound edges are what make 11d's question ("which records point at
    // that decision") an aggregation rather than a restatement of one history
    // line.
    let caching = call(
        executor,
        "create_node",
        json!({
            "content": "the caching layer depends on the rendering decision",
            "node_type": "text",
        }),
    )
    .await;
    let caching_id = bare(caching["id"].as_str().unwrap()).to_string();
    call(
        executor,
        "create_relationship",
        json!({
            "from_id": caching_id,
            "to_id": decision_id,
            "relationship_type": "mentions",
        }),
    )
    .await;

    (decision_id, task_id)
}

/// 11d's ideal call is accepted AND returns the linked node.
///
/// The scenario asserts `get_related_nodes` fired at least once. This proves
/// that a model which made that call would have been answered — the traversal
/// resolves the edge 11c recorded and comes back with the decision node.
///
/// So 11d's across-the-board failure is NOT the tool refusing the traversal,
/// and not the edge failing to persist. Both work. What remains is the model
/// choosing not to traverse, which is a genuine behavioural finding rather than
/// a harness defect — and the history-rendering side explains why it chooses
/// that way: the answer is already stated in the history it was given.
#[tokio::test(flavor = "multi_thread")]
async fn scenario_11d_ideal_traversal_is_accepted_and_finds_the_link() {
    let (executor, _tmp) = make_executor().await;
    let (decision_id, task_id) = seed_scenario_11(&executor).await;

    let out = call(
        &executor,
        "get_related_nodes",
        json!({ "id": task_id, "relationship_type": "mentions" }),
    )
    .await;

    assert!(
        out["count"].as_u64().unwrap() >= 1,
        "the traversal was accepted but came back EMPTY, which would make 11d \
         unwinnable in substance even though the call succeeds: {out:?}"
    );
    let found = out["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| bare(n["id"].as_str().unwrap_or("")) == decision_id);
    assert!(
        found,
        "the traversal returned nodes but not the decision 11c linked — the \
         edge resolves to something other than what the prompt asks for: {out:?}"
    );
}

/// The same traversal with NO `relationship_type` argument also finds the link.
///
/// Pins the default the tool documents (`mentions`). A model that omits the
/// argument entirely — the most likely spelling of the ideal call, since the
/// prompt names no relation — must still be answered. If the default ever
/// drifts, 11d would become unwinnable for that spelling while the explicit
/// one above kept passing, and the matrix would report it as a model failure.
#[tokio::test(flavor = "multi_thread")]
async fn scenario_11d_traversal_without_an_explicit_relation_uses_the_documented_default() {
    let (executor, _tmp) = make_executor().await;
    let (decision_id, task_id) = seed_scenario_11(&executor).await;

    let out = call(&executor, "get_related_nodes", json!({ "id": task_id })).await;

    let found = out["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| bare(n["id"].as_str().unwrap_or("")) == decision_id);
    assert!(
        found,
        "omitting relationship_type must fall back to 'mentions' as the tool's \
         description promises, or the most natural spelling of 11d's ideal call \
         silently returns nothing: {out:?}"
    );
}

/// Seed 8a..8d's graph state: two custom types and one instance of each.
async fn seed_scenario_8(executor: &GraphToolExecutor) -> (String, String) {
    let decision_schema = call(
        executor,
        "create_schema",
        json!({
            "name": "Architecture Decision",
            "fields": [
                {"name": "decision", "type": "text"},
                {"name": "decided_by", "type": "text"},
            ],
        }),
    )
    .await;
    let cycle_schema = call(
        executor,
        "create_schema",
        json!({
            "name": "Planning Cycle",
            "fields": [
                {"name": "cycle_name", "type": "text"},
                {"name": "end_date", "type": "date"},
            ],
        }),
    )
    .await;

    let decision_type = decision_schema["schemaId"].as_str().unwrap().to_string();
    let cycle_type = cycle_schema["schemaId"].as_str().unwrap().to_string();

    call(
        executor,
        "create_node",
        json!({
            "content": "event-based cache clearing",
            "node_type": decision_type,
            "field_values": {"decided_by": "Priya"},
        }),
    )
    .await;
    call(
        executor,
        "create_node",
        json!({
            "content": "Harbour",
            "node_type": cycle_type,
            "field_values": {"end_date": "2026-08-30"},
        }),
    )
    .await;

    (decision_type, cycle_type)
}

/// 8e's ideal call is accepted and returns the decision, not the cycle.
///
/// The scenario asserts `search_nodes` exactly once for "Run through those
/// calls for me". This proves a type-filtered read of the decision type is
/// both legal and correctly discriminating — the cycle instance seeded
/// alongside it does not come back.
///
/// So 8e's failure is not the read being impossible. What the prompt leaves
/// open is which type "those calls" refers to, and that is a wording question,
/// not a plumbing one.
#[tokio::test(flavor = "multi_thread")]
async fn scenario_8e_ideal_cross_type_read_is_accepted_and_discriminates() {
    let (executor, _tmp) = make_executor().await;
    let (decision_type, _cycle_type) = seed_scenario_8(&executor).await;

    let out = call(
        &executor,
        "search_nodes",
        json!({ "node_type": decision_type }),
    )
    .await;

    let text = serde_json::to_string(&out).unwrap();
    assert!(
        text.contains("event-based cache clearing"),
        "the type-filtered read must return the decision instance, or 8e has \
         no ideal call that answers it: {out:?}"
    );
    assert!(
        !text.contains("Harbour"),
        "the read returned the PLANNING CYCLE as well, so a type filter does \
         not discriminate between the two types 8a/8b created — 8e would then \
         be unwinnable as a targeted read: {out:?}"
    );
}

/// Scenario 6's ideal `update_node` is accepted and persists the state change.
///
/// The scenario asserts the `[resolve_query, update_node]` subsequence with
/// `minProperties: 1`. This covers the second half — that the write itself,
/// carrying the sign-off value, reaches storage and is reported as having
/// persisted a property.
///
/// It deliberately does NOT route through `resolve_query`. The audit question
/// for 6 is whether reaching the correct end state by a different path should
/// score red, and answering that requires knowing the direct write works. It
/// does.
#[tokio::test(flavor = "multi_thread")]
async fn scenario_6_ideal_update_is_accepted_and_persists_the_state_change() {
    let (executor, _tmp) = make_executor().await;

    let schema = call(
        &executor,
        "create_schema",
        json!({
            "name": "Feature Write-up",
            "fields": [
                {"name": "signed_off", "type": "boolean"},
                {"name": "estimated_days", "type": "number"},
            ],
        }),
    )
    .await;
    let writeup_type = schema["schemaId"].as_str().unwrap().to_string();

    let created = call(
        &executor,
        "create_node",
        json!({
            "content": "offline sync",
            "node_type": writeup_type,
            "field_values": {"signed_off": false, "estimated_days": 5},
        }),
    )
    .await;
    let node_id = bare(created["id"].as_str().unwrap()).to_string();

    let updated = call(
        &executor,
        "update_node",
        json!({
            "id": node_id,
            "field_values": {"signed_off": true},
        }),
    )
    .await;

    // `minProperties: 1` scores off the write reporting a persisted property.
    // A call that returned `updated: true` with no property count is the exact
    // shape the assertion exists to catch, so assert the count, not the flag.
    let persisted = updated
        .get("property_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert!(
        persisted >= 1,
        "the ideal update reported {persisted} persisted properties — scenario 6's \
         minProperties: 1 could not be satisfied by any model, making the \
         scenario unwinnable regardless of how it resolved the node: {updated:?}"
    );
}

/// 11d's ideal call enumerates BOTH inbound edges in a single traversal.
///
/// 11d asserts `noRetry` with `minCalls: 1` — at least one traversal, and no
/// blind repeat. That is only a fair assertion if one call can actually answer
/// the prompt: if enumerating two links required two `get_related_nodes` calls,
/// a correct model would trip the no-repeat half and score red for doing the
/// right thing.
///
/// This proves it does not. One call against the decision returns both the
/// task and the caching note, so one traversal is the expected shape and the
/// scenario's own note about the two-call false positive stays as narrow as it
/// claims.
#[tokio::test(flavor = "multi_thread")]
async fn scenario_11d_one_traversal_enumerates_every_inbound_link() {
    let (executor, _tmp) = make_executor().await;
    let (decision_id, task_id) = seed_scenario_11(&executor).await;

    let out = call(&executor, "get_related_nodes", json!({ "id": decision_id })).await;

    let ids: Vec<String> = out["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| bare(n["id"].as_str().unwrap_or("")).to_string())
        .collect();

    assert!(
        ids.contains(&task_id),
        "the rebuild task must come back from a single traversal of the \
         decision: {out:?}"
    );
    assert_eq!(
        ids.len(),
        2,
        "exactly the two records pointing at the decision must come back from ONE \
         call — fewer means enumerating them would take two traversals and 11d's \
         noRetry half would red out a correct model; more means the traversal is \
         returning nodes nothing linked: {out:?}"
    );
}

/// Scenario 12's ideal read-then-compare-then-write chain is accepted, and the
/// read actually surfaces the values the comparison needs.
///
/// 12 names its target by a COMPARATIVE ("whichever is the biggest job"), so
/// the model must rank three estimates before it can choose an id. It does NOT
/// replace the decomposition coverage scenario 6 gave up — the estimates are
/// all inline in the rendered history, so the ranking can be done in-context
/// and a read is not forced. See the group header in
/// scripts/eval/fixtures/agent-matrix.ts, and #2248 for the gap that remains.
///
/// A read is one legitimate route to that ranking, though, and it is the route
/// that could quietly make the scenario unwinnable. If `search_nodes` returned only
/// titles, or truncated to fewer than the three instances, no amount of model
/// capability would let it pick the largest — the values would simply not be
/// there to compare, and 12 would red-line correct behavior the way 6 did. So
/// this asserts the read surfaces all three instances AND their estimates,
/// before asserting the write on the winner persists.
///
/// Uses plain `search_nodes` rather than `search_semantic`: `make_executor`
/// builds an executor with no embedding service and no inference engine, so a
/// semantic read is not available here — and the scenario's own diagnostic
/// names `search_nodes` for the same reason.
#[tokio::test(flavor = "multi_thread")]
async fn scenario_12_ideal_comparative_chain_is_accepted_and_the_read_carries_the_values() {
    let (executor, _tmp) = make_executor().await;

    let schema = call(
        &executor,
        "create_schema",
        json!({
            "name": "Feature Write-up",
            "fields": [
                {"name": "signed_off", "type": "boolean"},
                {"name": "estimated_days", "type": "number"},
            ],
        }),
    )
    .await;
    let writeup_type = schema["schemaId"].as_str().unwrap().to_string();

    // The three instances 12's setup turns create. The largest estimate is
    // deliberately NOT the most recently created one, so a model that picks
    // "the last thing we talked about" lands on the wrong node and the
    // scenario's `contentMatches` clause catches it.
    let mut ids = Vec::new();
    for (title, days) in [
        ("checkout rewrite", 9),
        ("search indexer", 21),
        ("audit log export", 4),
    ] {
        let created = call(
            &executor,
            "create_node",
            json!({
                "content": title,
                "node_type": writeup_type,
                "field_values": {"signed_off": false, "estimated_days": days},
            }),
        )
        .await;
        ids.push((title, bare(created["id"].as_str().unwrap()).to_string()));
    }

    // The read the comparison depends on.
    let found = call(
        &executor,
        "search_nodes",
        // `"*"` is the enumerate form (`search_ops::normalize_enumerate_query`):
        // scope to the type, apply no title keyword filter. A literal phrase
        // here would be matched against node TITLES with `contains` when no
        // property filters are supplied, which returns nothing — the values the
        // comparison needs would be invisible for a reason that has nothing to
        // do with the model.
        json!({"query": "*", "node_type": writeup_type}),
    )
    .await;
    let text = serde_json::to_string(&found).unwrap();

    for (title, _) in &ids {
        assert!(
            text.contains(title),
            "the read did not surface '{title}', so fewer than three instances are \
             visible and the comparison scenario 12 asks for cannot be made — the \
             scenario would be unwinnable by construction: {found:?}"
        );
    }
    for days in ["9", "21", "4"] {
        assert!(
            text.contains(days),
            "the read did not surface the estimate {days}, so the values the \
             comparative ranges over are not in the result — a model could not \
             pick the largest from this no matter how capable: {found:?}"
        );
    }

    // The write on the winner (the 21-day search indexer).
    let target = ids
        .iter()
        .find(|(title, _)| *title == "search indexer")
        .map(|(_, id)| id.clone())
        .unwrap();

    let updated = call(
        &executor,
        "update_node",
        json!({
            "id": target,
            "field_values": {"signed_off": true},
        }),
    )
    .await;

    // Same reasoning as scenario 6: the end clause scores off a persisted
    // property value, so assert the count rather than the `updated` flag.
    let persisted = updated
        .get("property_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert!(
        persisted >= 1,
        "the ideal update reported {persisted} persisted properties — scenario 12's \
         `hasPropertyValue` clause could not be satisfied by any model: {updated:?}"
    );
}

/// The OTHER route to scenario 12's answer — sort descending, take one — is
/// accepted, with no filter needed.
///
/// 12 is scored on its end state, so both routes must reach it or the scenario
/// silently penalises the better one. Enumerate-then-compare (proved above)
/// puts the comparison in the model; `sorting` + `limit: 1` pushes it into the
/// query. The second is arguably the stronger answer.
///
/// The call carried a `gt 0` predicate that was true of every instance and
/// existed only to route around a bug: a filterless search dropped `sorting`
/// on the floor. `run_node_query` now routes on the presence of `sorting`
/// rather than on filters alone, so the plain phrasing — the one a model
/// actually reaches for — works, and the compensating filter is gone.
///
/// Asserts the top result is the 21-day instance specifically, not merely that
/// something came back: a sort that silently ignored its direction would return
/// a different node and still look like a working call — which is exactly how
/// the bug hid.
#[tokio::test(flavor = "multi_thread")]
async fn scenario_12_sorted_single_result_route_is_also_accepted() {
    let (executor, _tmp) = make_executor().await;

    let schema = call(
        &executor,
        "create_schema",
        json!({
            "name": "Feature Write-up",
            "fields": [
                {"name": "signed_off", "type": "boolean"},
                {"name": "estimated_days", "type": "number"},
            ],
        }),
    )
    .await;
    let writeup_type = schema["schemaId"].as_str().unwrap().to_string();

    for (title, days) in [
        ("checkout rewrite", 9),
        ("search indexer", 21),
        ("audit log export", 4),
    ] {
        call(
            &executor,
            "create_node",
            json!({
                "content": title,
                "node_type": writeup_type,
                "field_values": {"signed_off": false, "estimated_days": days},
            }),
        )
        .await;
    }

    let found = call(
        &executor,
        "search_nodes",
        json!({
            "query": "*",
            "node_type": writeup_type,
            "sorting": [{"field": "estimated_days", "direction": "desc"}],
            "limit": 1,
        }),
    )
    .await;

    let text = serde_json::to_string(&found).unwrap();
    assert!(
        text.contains("search indexer"),
        "sorting by estimated_days descending with limit 1 did not return the \
         21-day instance, so the query-side route to scenario 12's answer does \
         not work and the scenario would score a correct model red for taking \
         it: {found:?}"
    );
}

/// REGRESSION: a filterless search must honour `sorting`.
///
/// `run_node_query` used to branch on `filters.is_empty()` alone, and the
/// filterless branch calls `node_ops::query_nodes`, whose input struct has no
/// sorting field at all — so the argument was parsed, accepted, and dropped.
/// It now routes on the presence of `sorting` too, reaching
/// `query_ops::execute_query`, which honours it.
///
/// The old failure mode was the dangerous kind: no error, no warning, a
/// plausible node returned. A model that asks for "the longest-running one" via
/// `sorting: [{estimated_days, desc}], limit: 1` was handed whichever row came
/// first and had no way to tell it was not sorted. Asking for a superlative is
/// a natural way to answer a comparative question, so this is reachable from
/// ordinary phrasing rather than an exotic call shape — which is why the
/// assertion below names the 21-day node rather than merely checking that
/// something came back.
#[tokio::test(flavor = "multi_thread")]
async fn filterless_search_honours_sorting() {
    let (executor, _tmp) = make_executor().await;

    let schema = call(
        &executor,
        "create_schema",
        json!({
            "name": "Feature Write-up",
            "fields": [{"name": "estimated_days", "type": "number"}],
        }),
    )
    .await;
    let writeup_type = schema["schemaId"].as_str().unwrap().to_string();

    // Insertion order is deliberately not sort order: the first-inserted node
    // has the SMALLEST estimate, so "returns the first row" and "returns the
    // largest" are distinguishable outcomes.
    for (title, days) in [("small", 4), ("large", 21)] {
        call(
            &executor,
            "create_node",
            json!({
                "content": title,
                "node_type": writeup_type,
                "field_values": {"estimated_days": days},
            }),
        )
        .await;
    }

    let found = call(
        &executor,
        "search_nodes",
        json!({
            "query": "*",
            "node_type": writeup_type,
            "sorting": [{"field": "estimated_days", "direction": "desc"}],
            "limit": 1,
        }),
    )
    .await;

    let text = serde_json::to_string(&found).unwrap();
    assert!(
        text.contains("large") && !text.contains("small"),
        "a filterless search sorted descending by `estimated_days` with \
         `limit: 1` must return the 21-day node. Getting the 4-day node means \
         `sorting` is being dropped again — the call reports success either \
         way, so nothing else would catch it: {found:?}"
    );
}

/// A keyword means the same thing whether or not the search is sorted, EVEN
/// when the type's title is built from a template rather than from content.
///
/// Sorting changes which backend runs the search, so the keyword must not
/// change meaning across that boundary. Both paths filter on TITLE, and this
/// pins that with the fixture where title and content genuinely DIVERGE: a
/// `title_template` builds the title from properties, so `compute_title` never
/// falls back to the content. Matching content on the sorted path here returned
/// ZERO of two real matches while reporting success — the dropped-`sorting` bug
/// traded for a dropped-keyword one.
///
/// A schema with no template cannot catch that: title is derived from content
/// there, so the two filters coincide and the test passes either way.
#[tokio::test(flavor = "multi_thread")]
async fn a_keyword_means_the_same_sorted_or_not_under_a_title_template() {
    let (executor, _tmp) = make_executor().await;

    // The title comes from `venue`; the content deliberately shares no word
    // with it, so a content-matching search finds NONE of these nodes.
    let schema = call(
        &executor,
        "create_schema",
        json!({
            "name": "Booking",
            "fields": [
                {"name": "venue", "type": "text"},
                {"name": "head_count", "type": "number"},
            ],
            "title_template": "{venue} hall",
        }),
    )
    .await;
    let booking_type = schema["schemaId"].as_str().unwrap().to_string();

    // The largest head_count does NOT match the keyword, so "narrowed then
    // sorted" and "sorted with the keyword dropped" name different nodes.
    for (venue, head_count) in [("payments", 40), ("payments", 120), ("indexer", 300)] {
        call(
            &executor,
            "create_node",
            json!({
                "content": "body text sharing no word with the title",
                "node_type": booking_type,
                "field_values": {"venue": venue, "head_count": head_count},
            }),
        )
        .await;
    }

    let unsorted = call(
        &executor,
        "search_nodes",
        json!({"query": "payments", "node_type": booking_type, "limit": 10}),
    )
    .await;
    assert_eq!(
        unsorted["count"], 2,
        "the unsorted keyword search must match both `payments hall` bookings \
         before the sorted one is meaningful to compare against: {unsorted:?}"
    );

    let sorted = call(
        &executor,
        "search_nodes",
        json!({
            "query": "payments",
            "node_type": booking_type,
            "sorting": [{"field": "head_count", "direction": "desc"}],
            "limit": 10,
        }),
    )
    .await;
    assert_eq!(
        sorted["count"], 2,
        "adding `sorting` must not change WHICH nodes a keyword matches. Zero \
         here means the sorted path matched on content instead of title, \
         silently dropping both real matches while reporting success: {sorted:?}"
    );

    // And having matched the right two, it orders them.
    let top = serde_json::to_string(&sorted["nodes"][0]).unwrap();
    assert!(
        top.contains("120"),
        "the 120-head_count booking must sort ahead of the 40 one: {sorted:?}"
    );
}

/// Scenario 13's ideal lookup-then-write is accepted, and the lookup actually
/// discriminates on the seeded property.
///
/// 13 is the scenario #2248 asked for: its referent ("the incident Rowan was on
/// call for") is seeded out of band, so nothing about it reaches the rendered
/// prompt and the model MUST read before it can write. The daemon test
/// `scenario_13_seeded_referent_is_absent_from_history` proves the absence
/// side; this proves the other half, that a model which does the right thing is
/// not refused.
///
/// The lookup is the fragile half and the reason this test exists. Scenario 13
/// filters on a property value rather than a title, and a filtered
/// `search_nodes` routes through `QueryService` (`run_node_query`'s non-empty
/// `filters` branch) rather than the title-listing path — a genuinely different
/// code path from the one every other scenario exercises. If that filter did
/// not discriminate, 13 would be unwinnable by construction and would red-line
/// correct behavior exactly as scenarios 6 and 12 did.
///
/// Asserts the filter returns the ONE matching incident and NOT the other two:
/// a filter that silently matched everything would still "find" the target and
/// still look like a working call.
#[tokio::test(flavor = "multi_thread")]
async fn scenario_13_ideal_lookup_then_write_is_accepted() {
    let (executor, _tmp) = make_executor().await;

    // The type and instances `seedIncidents` establishes in the fixture. Seeded
    // here through the tool path rather than the CLI: this test is about
    // whether the AGENT's calls are accepted against that state, and the tool
    // path produces the same rows.
    let schema = call(
        &executor,
        "create_schema",
        json!({
            "name": "incident_report",
            "fields": [
                {"name": "on_call", "type": "text"},
                {"name": "resolved", "type": "boolean"},
            ],
        }),
    )
    .await;
    let incident_type = schema["schemaId"].as_str().unwrap().to_string();

    let mut target_id = String::new();
    for (title, on_call) in [
        ("checkout latency spike", "dana"),
        ("search index corruption", "rowan"),
        ("auth token expiry storm", "sam"),
    ] {
        let created = call(
            &executor,
            "create_node",
            json!({
                "content": title,
                "node_type": incident_type,
                "field_values": {"on_call": on_call, "resolved": false},
            }),
        )
        .await;
        if on_call == "rowan" {
            target_id = bare(created["id"].as_str().unwrap()).to_string();
        }
    }

    // The lookup: filter on the seeded property the prompt names.
    let found = call(
        &executor,
        "search_nodes",
        json!({
            "query": "*",
            "node_type": incident_type,
            "filters": [{"property": "on_call", "operator": "equals", "value": "rowan"}],
        }),
    )
    .await;

    let text = serde_json::to_string(&found).unwrap();
    assert!(
        text.contains("search index corruption"),
        "the on_call filter did not return the incident it identifies, so \
         scenario 13's only route to the id does not work and the scenario is \
         unwinnable by construction: {found:?}"
    );
    for other in ["checkout latency spike", "auth token expiry storm"] {
        assert!(
            !text.contains(other),
            "the on_call filter also returned '{other}', so it does not \
             discriminate — 13 would score a model green for a lookup that \
             cannot actually identify the target: {found:?}"
        );
    }

    // The write on the resolved target.
    let updated = call(
        &executor,
        "update_node",
        json!({
            "id": target_id,
            "field_values": {"resolved": true},
        }),
    )
    .await;

    // Same reasoning as scenarios 6 and 12d: the end clause scores off a
    // persisted property, so assert the count rather than the `updated` flag.
    let persisted = updated
        .get("property_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert!(
        persisted >= 1,
        "the ideal update reported {persisted} persisted properties — scenario \
         13's minProperties clause could not be satisfied by any model: {updated:?}"
    );
}

/// A sorted search with NO `node_type` orders by the real property values.
///
/// Sorting now routes a filterless search to `QueryService`, which makes the
/// typeless shape — `query: "*"` and no type, the plainest "sort everything by
/// X" a model can write — newly reachable. Under a wildcard the properties
/// namespace differs row by row, so a fixed `$.<field>` path is NULL for EVERY
/// row. SQL still ORDERs and LIMITs on that NULL key, and the in-Rust re-sort
/// only ever sees what survived the LIMIT: measured at `limit: 3`, the three
/// seeded nodes were cut entirely and three unrelated schema-registry rows came
/// back reported as a success.
///
/// The small limit is the whole point — a limit large enough to admit every row
/// hides this, because then the re-sort receives the full set and repairs the
/// order after the fact.
#[tokio::test(flavor = "multi_thread")]
async fn a_typeless_sorted_search_orders_by_the_property() {
    let (executor, _tmp) = make_executor().await;

    let schema = call(
        &executor,
        "create_schema",
        json!({
            "name": "Feature Write-up",
            "fields": [{"name": "estimated_days", "type": "number"}],
        }),
    )
    .await;
    let writeup_type = schema["schemaId"].as_str().unwrap().to_string();

    for (title, days) in [("small", 4), ("mid", 9), ("large", 21)] {
        call(
            &executor,
            "create_node",
            json!({
                "content": title,
                "node_type": writeup_type,
                "field_values": {"estimated_days": days},
            }),
        )
        .await;
    }

    // No `node_type`, and a limit smaller than the total row count.
    let found = call(
        &executor,
        "search_nodes",
        json!({
            "query": "*",
            "sorting": [{"field": "estimated_days", "direction": "desc"}],
            "limit": 3,
        }),
    )
    .await;

    let text = serde_json::to_string(&found).unwrap();
    assert!(
        text.contains("large") && text.contains("mid") && text.contains("small"),
        "a typeless sorted search must rank the seeded nodes by their real \
         `estimated_days`. Missing them means the wildcard ORDER BY key was \
         NULL for every row, so LIMIT cut the matches before any re-sort could \
         restore them — and the call still reported success: {found:?}"
    );

    let top = serde_json::to_string(&found["nodes"][0]).unwrap();
    assert!(
        top.contains("large"),
        "the 21-day node must sort first under `desc`: {found:?}"
    );
}
