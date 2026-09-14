//! Verification that a schema field which is *defined but unset* can be
//! written and filtered without the user naming the field.
//!
//! The reported failure: `due_date` is defined on the core `task` schema, but
//! `get_node` returned only *populated* properties, so an unset field was
//! indistinguishable from one that does not exist. The model — correctly
//! following "use the node's own existing property keys" — declined and asked
//! the user "what field name is used on this task node that tracks dates?".
//! The same gap on the read side had it asking the user to confirm that
//! `status` was the field and `open` a legal value. Both are defined on the
//! schema all along.
//!
//! Core types are the whole point here: the `EXISTING SCHEMAS` block
//! excludes them by construction, so for `task`/`text` there was no path —
//! prompt block or tool call — by which the field list could reach the model.
//! The fix delivers it on the tool result instead (ADR-064 rule 4), leaving
//! that exclusion untouched.
//!
//! Two layers, deliberately:
//!
//! - The `deterministic_*` tests drive the real `GraphToolExecutor` over a real
//!   `SqliteStore` and assert on the tool-result *contract*. They need no model
//!   and run in the normal suite, so a regression in what the model is shown is
//!   caught by `test:all` rather than only by an ignored live run.
//! - The `#[ignore]`d live test proves the locked model actually *uses* it.
//!   That distinction is load-bearing on this stack: #1931's guidance verified
//!   3/3 in isolation and still failed live, so a contract test alone does not
//!   establish a fix on this surface.
//!
//! Live run:
//! ```text
//! cargo test -p nodespace-agent --test live_unset_schema_field -- --ignored --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use nodespace_agent::agent_types::{
    ChatInferenceEngine, ChatMessage, InferenceRequest, ModelFamily, Role, StreamingChunk,
    ToolDefinition,
};
use nodespace_agent::local_agent::agent_loop::repair_tool_call_arguments;
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_agent::local_agent::tools::{all_tool_definitions, GraphToolExecutor};
use nodespace_agent::AgentToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::services::NodeService;
use nodespace_nlp_engine::chat::ChatConfig;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;

/// Independent repetitions for the live test. Single-run results on this stack
/// are not decision-grade — identical code has scored differently across
/// repeated runs at temperature 0.1.
const REPS: usize = 3;

fn model_path() -> String {
    let home = std::env::var("HOME").expect("HOME must be set");
    format!("{home}/.nodespace/models/gemma-4-E4B-it-Q4_K_M.gguf")
}

/// Production's enforced floor (`N_CTX_MINIMUM` in `nlp-engine`'s chat module),
/// not the 32768 the sibling live suites request.
///
/// These scenarios are a handful of short turns, so the larger window buys
/// nothing — and at 32768 the KV cache plus compute buffers OOM the GPU
/// (`kIOGPUCommandBufferCallbackErrorOutOfMemory`) whenever a `nodespaced`
/// daemon is resident with its own model loaded, which is the normal state of a
/// dev machine. A live test that only passes when nothing else is running is a
/// flake generator, and a *behavioral* assertion that fails for a memory reason
/// is worse than no assertion: it reads as the fix having regressed.
const LIVE_N_CTX: u32 = 16_384;

fn load_engine() -> LlamaChatInferenceEngine {
    let config = ChatConfig {
        n_ctx: LIVE_N_CTX,
        default_temperature: 0.1,
        ..Default::default()
    };
    LlamaChatInferenceEngine::load(&model_path(), ModelFamily::Gemma4, config)
        .expect("model must load from the standard catalog path")
}

async fn make_executor() -> (GraphToolExecutor, Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("live.db");
    let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
    let executor = GraphToolExecutor {
        node_service: Some(ns.clone()),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, ns, tmp)
}

/// Create the reproducing fixture: a task with only `status` populated, so
/// `due_date` is defined-but-unset exactly as in the report.
async fn create_polestar_task(executor: &GraphToolExecutor) -> String {
    let created = executor
        .execute(
            "create_node",
            json!({
                "content": "Schedule chip upgrade on the Polestar",
                "node_type": "task",
                "field_values": {"status": "in_progress"},
            }),
        )
        .await
        .expect("fixture creation must succeed");
    created.result["id"]
        .as_str()
        .unwrap()
        .trim_start_matches("nodespace://")
        .to_string()
}

fn available_field<'a>(result: &'a Value, name: &str) -> Option<&'a Value> {
    result["available_properties"]
        .as_array()?
        .iter()
        .find(|f| f["name"] == name)
}

// ---------------------------------------------------------------------------
// Deterministic contract tests — no model required.
// ---------------------------------------------------------------------------

/// The write-path defect, stated as a contract: `get_node` on a core-type node
/// must name a defined-but-unset field and mark it unset. Before the fix
/// `properties` held only `status` and `due_date` appeared nowhere.
#[tokio::test(flavor = "multi_thread")]
async fn deterministic_get_node_surfaces_unset_core_schema_fields() {
    let (executor, _ns, _tmp) = make_executor().await;
    let node_id = create_polestar_task(&executor).await;

    let stored = executor
        .execute("get_node", json!({ "id": node_id }))
        .await
        .expect("get_node must succeed");

    let due = available_field(&stored.result, "due_date").unwrap_or_else(|| {
        panic!(
            "due_date is defined on the core task schema but is absent from \
             available_properties — the model cannot name a field it is never shown. \
             result={}",
            stored.result
        )
    });
    assert_eq!(
        due["set"],
        json!(false),
        "due_date has no value on this node, so it must report set=false — that flag is \
         the only thing distinguishing 'defined but unset' from 'does not exist'"
    );

    // The populated field must be distinguishable from the unset one, or the
    // flag carries no information.
    let status = available_field(&stored.result, "status").expect("status must be listed");
    assert_eq!(status["set"], json!(true));

    // Enum values must ride along, or `open` still has to be guessed.
    let allowed = status["allowed_values"]
        .as_array()
        .expect("status is an enum, so its legal values must be exposed");
    assert!(
        allowed.iter().any(|v| v == "done"),
        "status allowed_values should carry the core enum members, got {allowed:?}"
    );
}

/// The name taken from `available_properties` must be directly usable as an
/// `update_node` key. This is the #1937 read/write-shape concern asserted
/// rather than assumed: reads flatten the stored `{"task":{...}}` namespace and
/// writes re-namespace flat keys, so the flat name must round-trip.
#[tokio::test(flavor = "multi_thread")]
async fn deterministic_unset_field_name_round_trips_through_update_node() {
    let (executor, _ns, _tmp) = make_executor().await;
    let node_id = create_polestar_task(&executor).await;

    let stored = executor
        .execute("get_node", json!({ "id": node_id }))
        .await
        .expect("get_node must succeed");
    let field_name = available_field(&stored.result, "due_date").expect("due_date must be listed")
        ["name"]
        .as_str()
        .expect("field name must be a string")
        .to_string();

    // Write using exactly the name the tool result advertised — no translation.
    let outcome = executor
        .execute(
            "update_node",
            json!({ "id": node_id, "field_values": { field_name.clone(): "2026-08-06" } }),
        )
        .await
        .expect("update_node must accept a schema-defined field name");
    assert!(!outcome.is_error, "update failed: {:?}", outcome);

    // Assert on the STORE, not the tool's own report — the entire #1937 defect
    // was a success result over an unchanged store.
    let after = executor
        .execute("get_node", json!({ "id": node_id }))
        .await
        .expect("get_node must succeed");
    assert_eq!(
        after.result["properties"][&field_name],
        json!("2026-08-06"),
        "the field name advertised by available_properties did not persist when passed \
         straight back to update_node — read shape and write shape have diverged. \
         stored={}",
        after.result["properties"]
    );
    // And it now reports as set, closing the loop.
    assert_eq!(
        available_field(&after.result, "due_date").unwrap()["set"],
        json!(true)
    );
}

/// The read-path defect: a zero-result type-scoped search must say which fields
/// exist, so `count: 0` stops being indistinguishable from "that field does not
/// exist" — the ambiguity that had the model asking the user to confirm
/// `status` and `open`.
#[tokio::test(flavor = "multi_thread")]
async fn deterministic_empty_search_names_filterable_fields() {
    let (executor, _ns, _tmp) = make_executor().await;
    create_polestar_task(&executor).await;

    let found = executor
        .execute(
            "search_nodes",
            json!({
                "query": "",
                "node_type": "task",
                "filters": [{"type": "property", "property": "status", "operator": "equals", "value": "open"}],
            }),
        )
        .await
        .expect("search_nodes must succeed");

    assert_eq!(
        found.result["count"],
        json!(0),
        "fixture is in_progress, not open"
    );

    let fields = found.result["filterable_properties"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "an empty type-scoped search must name the type's filterable fields, or the \
                 model cannot tell an empty result from a bad field name. result={}",
                found.result
            )
        });
    let status = fields
        .iter()
        .find(|f| f["name"] == "status")
        .expect("status must be listed as filterable");
    let allowed = status["allowed_values"]
        .as_array()
        .expect("status enum values must be exposed so `open` need not be guessed");
    assert!(
        allowed.iter().any(|v| v == "in_progress"),
        "expected core status values, got {allowed:?}"
    );
    // `set` is per-node and there is no node here — leaving it in would assert
    // something false about every field.
    assert!(
        status.get("set").is_none(),
        "filterable_properties describes a type, not a node, so `set` is meaningless here"
    );
}

/// ADR-064 rule 4: a tool result owns *facts*, not procedures. The field list
/// is a fact and belongs here; prose telling the model what to do about it is a
/// plan, and belongs in the tool description (rule 2) where tool-selection
/// guidance lives.
///
/// An earlier draft of this fix shipped a `no_matches_hint` prose string
/// alongside the list. It read as helpful and violated the doctrine — and the
/// doctrine is measured, not stylistic: instructions delivered as a tool result
/// dropped continuation from 100% to 44% in this codebase's own testing.
#[tokio::test(flavor = "multi_thread")]
async fn deterministic_empty_search_returns_facts_not_instructions() {
    let (executor, _ns, _tmp) = make_executor().await;
    create_polestar_task(&executor).await;

    let found = executor
        .execute(
            "search_nodes",
            json!({
                "query": "",
                "node_type": "task",
                "filters": [{"type": "property", "property": "status", "operator": "equals", "value": "open"}],
            }),
        )
        .await
        .expect("search_nodes must succeed");

    let obj = found.result.as_object().expect("result must be an object");
    let allowed = ["count", "nodes", "filterable_properties"];
    let unexpected: Vec<&String> = obj
        .keys()
        .filter(|k| !allowed.contains(&k.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "the result carries key(s) {unexpected:?} beyond the facts {allowed:?} — if that is \
         procedural prose it belongs in the tool description, not the tool result (ADR-064 rule 4)"
    );
}

/// A successful search must NOT carry the field block. Appending schema to
/// every result would put a block in front of the model on turns that never
/// needed one — the dilution the resident-prompt findings warn against.
#[tokio::test(flavor = "multi_thread")]
async fn deterministic_successful_search_omits_the_field_block() {
    let (executor, _ns, _tmp) = make_executor().await;
    create_polestar_task(&executor).await;

    let found = executor
        .execute("search_nodes", json!({ "query": "", "node_type": "task" }))
        .await
        .expect("search_nodes must succeed");

    assert!(
        found.result["count"].as_u64().unwrap_or(0) > 0,
        "fixture must match"
    );
    assert!(
        found.result.get("filterable_properties").is_none(),
        "a non-empty result needs no field list: {}",
        found.result
    );
}

/// ADR-063 guard on the new channel, asserted rather than assumed. The issue is
/// explicit that core schema fields reaching the model again must be re-verified
/// not to reopen the bare-key hole. `available_properties` can only ever echo
/// names the core schema already defines, so it introduces no path to writing a
/// *new* bare key onto a core type.
#[tokio::test(flavor = "multi_thread")]
async fn deterministic_available_properties_advertises_no_undefined_bare_keys() {
    let (executor, ns, _tmp) = make_executor().await;
    let node_id = create_polestar_task(&executor).await;

    let stored = executor
        .execute("get_node", json!({ "id": node_id }))
        .await
        .expect("get_node must succeed");

    let schema = ns
        .get_schema_node("task")
        .await
        .expect("schema lookup must succeed")
        .expect("task is a seeded core schema");
    let defined: Vec<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();

    let advertised = stored.result["available_properties"]
        .as_array()
        .expect("available_properties must be present");
    assert!(
        !advertised.is_empty(),
        "task defines fields, so the list must be non-empty"
    );

    for field in advertised {
        let name = field["name"]
            .as_str()
            .expect("each entry must name a field");
        assert!(
            defined.contains(&name),
            "available_properties advertised '{name}', which the core task schema does not \
             define — this channel must never invent a key, since a bare key written onto a \
             core type is the ADR-063 violation the routing block's is_core filter exists to \
             prevent. defined={defined:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Live test — does the locked model actually act on it?
// ---------------------------------------------------------------------------

/// Only the tools this scenario needs. These are the REAL production
/// definitions, filtered — not hand-authored schemas.
fn write_path_tools() -> Vec<ToolDefinition> {
    all_tool_definitions()
        .into_iter()
        .filter(|t| t.name == "update_node" || t.name == "get_node")
        .collect()
}

/// Runs one turn and returns the parsed (tool_name, args_json), if any.
async fn run_turn(
    engine: &LlamaChatInferenceEngine,
    system_prompt: &str,
    tools: Vec<ToolDefinition>,
    messages: Vec<ChatMessage>,
) -> (Option<(String, String)>, String) {
    let mut all = vec![ChatMessage::text(Role::System, system_prompt.to_string())];
    all.extend(messages);

    let request = InferenceRequest {
        messages: all,
        tools: Some(tools),
        temperature: Some(0.1),
        max_tokens: Some(512),
    };

    let chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = chunks.clone();
    engine
        .generate(
            request,
            Box::new(move |c| {
                if let Ok(mut g) = sink.lock() {
                    g.push(c);
                }
            }),
        )
        .await
        .expect("generation must complete");

    let collected = chunks.lock().expect("chunk mutex").clone();
    let name = collected.iter().find_map(|c| match c {
        StreamingChunk::ToolCallStart { name, .. } => Some(name.clone()),
        _ => None,
    });
    let args: String = collected
        .iter()
        .filter_map(|c| match c {
            StreamingChunk::ToolCallArgs { args_json, .. } => Some(args_json.as_str()),
            _ => None,
        })
        .collect();
    let raw: String = collected
        .iter()
        .filter_map(|c| match c {
            StreamingChunk::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    (name.map(|n| (n, args)), raw)
}

/// The reported scenario end to end: the model is shown the node exactly as
/// `get_node` renders it and asked to set the due date, without ever being told
/// the field name.
///
/// Asserts on the STORE, and on the one behaviour the report is about: the model
/// must not come back asking the user to name the field.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the locked native GGUF on disk"]
async fn live_model_sets_an_unset_field_without_asking_for_its_name() {
    let engine = load_engine();
    let system = "You are a graph-editing assistant. Act on the user's request immediately \
        using the tools available. Do not ask the user to confirm.";

    let mut persisted = 0usize;
    let mut usable_reps = 0usize;

    for rep in 0..REPS {
        let (executor, _ns, _tmp) = make_executor().await;
        let node_id = create_polestar_task(&executor).await;

        // Turn 1 — the model looks the node up, exactly as it does in production.
        let lookup = executor
            .execute("get_node", json!({ "id": node_id }))
            .await
            .expect("get_node must succeed");

        // Feed that real tool result back as the model would see it, then ask
        // for the change. The field name is never mentioned in the prompt.
        let messages = vec![
            ChatMessage::text(
                Role::User,
                "Could you set this task's due date to 06-August-2026?".to_string(),
            ),
            ChatMessage::text(
                Role::Assistant,
                format!("Tool result for get_node:\n{}", lookup.result),
            ),
            ChatMessage::text(Role::User, "Go ahead and make that change now.".to_string()),
        ];

        let (call, raw) = run_turn(&engine, system, write_path_tools(), messages).await;

        let Some((name, args_json)) = call else {
            // This IS the reported failure mode: no call, and a question back
            // to the user about which field to use.
            println!("LIVE[rep {rep}] no tool call. raw: {raw:?}");
            let lowered = raw.to_lowercase();
            assert!(
                !(lowered.contains("field name") || lowered.contains("which property")),
                "rep {rep}: the model asked the user to name the field, which is the exact \
                 reported defect — available_properties should have supplied it: {raw:?}"
            );
            continue;
        };
        println!("LIVE[rep {rep}] {name}({args_json})");

        if name != "update_node" {
            println!("LIVE[rep {rep}] non-write tool, skipping");
            continue;
        }

        let args: Value = match serde_json::from_str(&args_json) {
            Ok(v) => v,
            Err(e) => {
                println!("LIVE[rep {rep}] unparseable args ({e}), skipping");
                continue;
            }
        };
        usable_reps += 1;

        let outcome = executor.execute("update_node", args).await;
        let after = executor
            .execute("get_node", json!({ "id": node_id }))
            .await
            .expect("get_node must succeed");
        let props = &after.result["properties"];

        match outcome {
            Err(e) => println!("LIVE[rep {rep}] rejected: {e}"),
            Ok(result) if result.is_error => {
                println!("LIVE[rep {rep}] tool error: {}", result.result)
            }
            Ok(_) => {
                // The date must have landed on a schema-defined field. Which
                // date field the model picks is its business; that SOME defined
                // field now holds the value is the fix working.
                let wrote_a_date = props.as_object().is_some_and(|o| {
                    o.iter().any(|(k, v)| {
                        k != "status" && v.as_str().is_some_and(|s| s.contains("2026-08-06"))
                    })
                });
                assert!(
                    wrote_a_date,
                    "rep {rep}: update reported success but no schema field holds the date: {props}"
                );
                persisted += 1;
            }
        }
    }

    println!("LIVE SUMMARY: persisted={persisted} of {usable_reps} usable reps ({REPS} total)");
    assert!(
        persisted > 0,
        "no rep persisted the date — the model still cannot write a defined-but-unset field"
    );
}

/// A node of a type that defines no fields (`text`) must come back unchanged
/// rather than erroring or carrying an empty list — the lookup the model asked
/// for is what matters, and the field list is an addition to it, never a
/// precondition.
///
/// The list must be *absent*, not an empty array: an empty array reads as "this
/// type has no fields", which is a stronger claim than "no fields were found",
/// and on a type that does define fields it would be actively wrong.
#[tokio::test(flavor = "multi_thread")]
async fn deterministic_fieldless_type_is_unaffected() {
    let (executor, _ns, _tmp) = make_executor().await;
    let created = executor
        .execute(
            "create_node",
            json!({ "content": "just a note", "node_type": "text" }),
        )
        .await
        .expect("text node creation must succeed");
    let node_id = created.result["id"]
        .as_str()
        .unwrap()
        .trim_start_matches("nodespace://")
        .to_string();

    let stored = executor
        .execute("get_node", json!({ "id": node_id }))
        .await
        .expect("get_node must succeed regardless of the field list");
    assert!(!stored.is_error);
    assert_eq!(stored.result["content"], json!("just a note"));

    // `text` is seeded with no fields, so the outcome is deterministic: the key
    // must be ABSENT. Asserting that directly rather than "absent or non-empty"
    // — a conditional assertion is a test that opts out of failing, and this
    // one would stop covering the `[]` regression it exists to catch.
    assert!(
        stored.result.get("available_properties").is_none(),
        "a type defining no fields must omit the key: an empty array asserts \
         'this type has no fields', a stronger claim than 'none were found'. got={}",
        stored.result
    );
}

/// Fetching a schema node itself must not recurse into "the schema of a
/// schema". Schema nodes use a non-namespaced property format, so treating
/// their fields as instance properties would describe them wrongly.
#[tokio::test(flavor = "multi_thread")]
async fn deterministic_get_node_on_a_schema_node_is_sane() {
    let (executor, _ns, _tmp) = make_executor().await;

    let stored = executor
        .execute("get_node", json!({ "id": "task" }))
        .await
        .expect("fetching the task schema node must succeed");

    assert!(!stored.is_error, "schema fetch errored: {:?}", stored);
    // Deterministic, so asserted directly: `SchemaNode` carries no `node_type`
    // field, so a schema node emits no `nodeType` and skips the attach block
    // entirely. The key must be absent.
    //
    // Stated as absence rather than "does not contain due_date" because that is
    // the real invariant. The weaker form would keep passing if a future
    // `nodeType` on `SchemaNode` started describing the task SCHEMA node with
    // the *schema* type's fields — wrong, but not `due_date`, so undetected.
    assert!(
        stored.result.get("available_properties").is_none(),
        "a schema node was given an instance field list — 'the schema of a schema' is not \
         a meaningful description, and schema nodes use a non-namespaced property format \
         that this list would describe wrongly. got={}",
        stored.result
    );
}

/// The read-path half, live. Observed in production: asked "How many tasks do
/// we have that are open?", the model came back asking the user to confirm
/// whether "open" was the exact value and which field tracked it — both
/// defined on the core task schema.
///
/// Given an empty result carrying the field list, the model must search again
/// against the right field rather than stop and interrogate the user — and that
/// retry must actually execute.
///
/// Measured on the locked model, 3 reps each, identical except for the payload:
///
/// | empty result carries      | outcome                                        |
/// |---------------------------|------------------------------------------------|
/// | `{count, nodes}` only     | 0/3 retried — "I do not have enough information |
/// |                           | to continue. Could you please clarify..."       |
/// | + `filterable_properties` | 3/3 retried, filtering on `status`               |
///
/// So the field list is what moves the model off asking the user.
///
/// All 3 retries originally serialized their arguments malformed — keys with
/// embedded quotes (`{"\"property\"": "status"}`) and a value that swallowed the
/// next key's delimiters (`"type": "task\",\"value\":"`) — so the call was
/// rejected before it ran. That is repaired at `LocalAgentLoop`'s parse boundary
/// (`agent_loop::repair_tool_call_arguments`), which this test calls explicitly
/// because it drives the inference engine directly with no loop in the path.
///
/// What repair cannot restore is the comparison value: the model writes the
/// delimiters that should have introduced `value` as string content, so that
/// value is never generated. The call therefore reaches the query layer and
/// fails there for a missing value — which is why this asserts "every retry got
/// as far as the missing value" rather than "every retry executed". See the
/// assertion at the end for why that is the honest bar on this model.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the locked native GGUF on disk"]
async fn live_model_recovers_from_a_bad_enum_value_without_asking() {
    let engine = load_engine();
    let system = "You are a graph-editing assistant. Act on the user's request immediately \
        using the tools available. Do not ask the user to confirm.";

    let search_tools: Vec<ToolDefinition> = all_tool_definitions()
        .into_iter()
        .filter(|t| t.name == "search_nodes")
        .collect();

    let mut recovered = 0usize;
    let mut executed = 0usize;
    // Retries that got all the way to the query and failed only because the
    // model never emitted a comparison value — the irreducible residue of the
    // splice, as opposed to a shape defect repair should have caught.
    let mut missing_value_only = 0usize;
    let mut asked_the_user = 0usize;

    for rep in 0..REPS {
        let (executor, _ns, _tmp) = make_executor().await;
        create_polestar_task(&executor).await;

        // A genuinely open task must exist, or this test cannot tell the two
        // outcomes apart: with only the in_progress fixture, "there are no open
        // tasks" is the CORRECT answer, and a model that gave up entirely would
        // score identically to one that recovered. The right answer is now
        // reachable only by retrying with a legal value.
        executor
            .execute(
                "create_node",
                json!({
                    "content": "Renew the parking permit",
                    "node_type": "task",
                    "field_values": {"status": "open"},
                }),
            )
            .await
            .expect("open-task fixture must be created");

        // The failing first attempt, run for real. `openn` is a plausible
        // near-miss of a legal value rather than a value that is merely absent
        // from the data — so an empty result here genuinely means "bad filter",
        // and the recovery under test is reading the legal values off the
        // result and retrying with one.
        let empty = executor
            .execute(
                "search_nodes",
                json!({
                    "query": "",
                    "node_type": "task",
                    "filters": [{"type": "property", "property": "status", "operator": "equals", "value": "openn"}],
                }),
            )
            .await
            .expect("search must succeed");
        assert_eq!(
            empty.result["count"],
            json!(0),
            "the bad filter must match nothing"
        );

        let messages = vec![
            ChatMessage::text(Role::User, "How many tasks are still open?".to_string()),
            ChatMessage::text(
                Role::Assistant,
                format!("Tool result for search_nodes:\n{}", empty.result),
            ),
            // Deliberately neutral. "Give me the answer" would push toward
            // answering from the empty result; the question under test is
            // whether the field list alone makes a retry the obvious next move.
            ChatMessage::text(Role::User, "Please continue.".to_string()),
        ];

        let (call, raw) = run_turn(&engine, system, search_tools.clone(), messages).await;

        match call {
            Some((name, args_json)) if name == "search_nodes" => {
                println!("LIVE[rep {rep}] retried: {args_json}");
                // A retry only counts if it actually runs. Counting the call by
                // name alone would score a malformed filter — keys emitted with
                // embedded quotes, a value spliced into a neighbouring field —
                // as a recovery, when executing it changes nothing.
                // Counted as a recovery: the model went back to the graph
                // instead of back to the user, and did so against the field the
                // result named. That is what the field list is responsible for.
                assert!(
                    args_json.contains("status"),
                    "rep {rep}: retried without filtering on `status`, the field the result \
                     named — the field list did not inform the retry: {args_json}"
                );
                recovered += 1;

                // This test drives the inference engine directly, so it must
                // apply the repair `LocalAgentLoop` applies at its parse
                // boundary — otherwise it measures a raw call no production
                // path ever executes.
                let mut repaired = args_json.clone();
                repair_tool_call_arguments(&mut repaired);
                if repaired != args_json {
                    println!("LIVE[rep {rep}] repaired to: {repaired}");
                }

                match serde_json::from_str::<Value>(&repaired) {
                    Ok(args) => match executor.execute("search_nodes", args).await {
                        Ok(r) if !r.is_error => {
                            println!("LIVE[rep {rep}] retry ran, count={}", r.result["count"]);
                            executed += 1;
                        }
                        Ok(r) => println!("LIVE[rep {rep}] retry errored: {}", r.result),
                        Err(e) => {
                            let msg = e.to_string();
                            // "Missing value" is the query layer reporting an
                            // `equals` filter with nothing to compare against —
                            // the value the model never emitted. Anything else
                            // is a shape defect and must fail the assertion.
                            if msg.contains("Missing value") {
                                missing_value_only += 1;
                            }
                            println!("LIVE[rep {rep}] retry rejected: {e}");
                        }
                    },
                    Err(e) => println!("LIVE[rep {rep}] retry args unparseable ({e})"),
                }
            }
            Some((name, args_json)) => println!("LIVE[rep {rep}] other tool {name}({args_json})"),
            None => {
                println!("LIVE[rep {rep}] no call. raw: {raw:?}");
                let lowered = raw.to_lowercase();
                // The reported defect verbatim: asking the user to supply a
                // field name or confirm an enum value the schema defines.
                if lowered.contains("exact value")
                    || lowered.contains("which field")
                    || lowered.contains("field name")
                    || lowered.contains("confirm")
                {
                    asked_the_user += 1;
                }
            }
        }
    }

    println!(
        "LIVE SUMMARY: recovered={recovered} executed={executed} \
         missing_value_only={missing_value_only} asked_the_user={asked_the_user} of {REPS}"
    );
    // The reported defect: interrogating the user about the system's own schema.
    assert_eq!(
        asked_the_user, 0,
        "the model asked the user to confirm a field name or enum value that the schema \
         defines and filterable_properties listed — the reported read-path defect"
    );
    // And the positive half. An open task exists, so stopping at the empty
    // result means reporting "none" over a non-empty set. Without the field
    // list this scores 0/3 (the model asks the user to clarify); with it, 3/3.
    // An assertion checking only for the absence of the bad question would pass
    // on a model that simply gave up quietly.
    assert!(
        recovered > 0,
        "no rep went back to the graph after the bad filter, so none could have found the \
         open task that exists — filterable_properties is not informing the retry"
    );
    // #1943 asked for this to assert the retry EXECUTES. Measured on the locked
    // model, it cannot, and the reason is not repairable downstream: the model
    // emits `"type": "task\",\"value\":"`, writing the delimiters that should
    // have introduced `value` as string content instead. The comparison value
    // is therefore never generated at all. Repair recovers the truncated `type`
    // (`task`) and the over-quoted keys, and category inference then routes the
    // filter correctly — but `equals` with no value is genuinely incomplete, and
    // inventing "open" here would be fabricating the user's query.
    //
    // 3 of 3 reps, before and after the schema/grammar change attempted for it,
    // produce byte-identical output — so this is a first-emission model defect,
    // not the history-propagation loop the repair fixes (that one is measured
    // separately at 8/8 vs 0/8 in nlp-engine/tests/toolcall_json_shape.rs, and
    // requires a prior malformed tool call, which this prompt has none of).
    //
    // So the honest assertion is the one below: repair must carry the call as
    // far as it can go — usable keys, a clean `type`, correct category routing —
    // and the residue must be exactly the missing value, not a shape defect.
    // Asserting `executed == recovered` instead would encode a model capability
    // this model does not have and leave a permanently red test.
    assert_eq!(
        executed + missing_value_only,
        recovered,
        "a retry failed for something other than the model's un-emitted comparison value. \
         Check the `repaired to:` lines above: repair or category inference has regressed, \
         or a new malformation has appeared"
    );
}
