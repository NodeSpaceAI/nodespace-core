//! Measurement harness for the question #2185 asks first: does
//! `resolve_query`'s internal decomposition sub-call actually emit malformed
//! filter shapes?
//!
//! The concern is real in structure. The four argument repairs
//! (`repair_over_quoted_keys`, `repair_leaked_special_token_keys`,
//! `repair_spliced_object_values`, `repair_scalar_in_operator_values`) run at
//! the agent loop's tool-call parse boundary. `exec_resolve_query` makes a
//! *nested* inference call and deserializes its output locally, so none of
//! them cover it. Whether that gap matters is an empirical question about what
//! this particular prompt, on this particular model, actually produces — and
//! #2182's lesson was precisely that the assumed malformation and the measured
//! one differed. So this measures before anything is repaired.
//!
//! What it does NOT do: assert a pass/fail bar on model behavior. It drives
//! the real `exec_resolve_query` over a corpus of phrasings, captures every
//! decomposition's *raw* text via the existing `NODESPACE_PROMPT_DUMP` hook
//! (`nlp-engine`'s `chat/prompt_dump.rs` names `resolve_query` as a covered
//! caller, so this needs no new production instrumentation), classifies each
//! emitted filter object against the shapes the repairs exist for, and prints
//! a tally. The human reads the tally and decides.
//!
//! Measuring through the real tool — rather than re-deriving the decomposition
//! prompt here — is deliberate. A hand-rebuilt prompt is a different arm from
//! the one production runs, and an arm that does not reach its own question
//! looks like a clean result. The prompt under test is built exactly once, in
//! `exec_resolve_query`, and this drives that.
//!
//! Run:
//! ```text
//! cargo test -p nodespace-agent --test live_resolve_query_decomposition_shapes \
//!   -- --ignored --nocapture --test-threads=1
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use nodespace_agent::agent_types::{ChatInferenceEngine, ModelFamily};
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_agent::local_agent::tools::{extract_json_object, GraphToolExecutor};
use nodespace_agent::AgentToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::schema::handle_create_schema;
use nodespace_core::services::NodeService;
use nodespace_nlp_engine::chat::ChatConfig;
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::RwLock;

/// Independent repetitions per phrasing. Single-run numbers on this stack are
/// not decision-grade — identical code has scored differently across repeated
/// runs even at low temperature — and this test's entire output is a count.
const REPS: usize = 3;

/// Production's enforced floor (`N_CTX_MINIMUM`), matching the sibling live
/// suites. The decomposition prompt is a few hundred tokens; a larger window
/// buys nothing and risks OOM when a `nodespaced` daemon is resident.
const LIVE_N_CTX: u32 = 16_384;

fn model_path() -> String {
    std::env::var("E2E_MODEL").unwrap_or_else(|_| {
        let home = std::env::var("HOME").expect("HOME must be set");
        format!("{home}/.nodespace/models/gemma-4-E4B-it-Q4_K_M.gguf")
    })
}

fn load_engine() -> LlamaChatInferenceEngine {
    let config = ChatConfig {
        n_ctx: LIVE_N_CTX,
        // The production decomposition call pins temperature 0.0 in its own
        // `InferenceRequest`, which overrides this default. Set here anyway so
        // the engine's fallback matches rather than contradicts it.
        default_temperature: 0.0,
        ..Default::default()
    };
    LlamaChatInferenceEngine::load(&model_path(), ModelFamily::Gemma4, config)
        .expect("model must load from the standard catalog path")
}

async fn make_service() -> (Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("decomp.db");
    let mut store: Arc<SqliteStore> = Arc::new(SqliteStore::new(db_path).await.unwrap());
    let svc = Arc::new(NodeService::new(&mut store).await.unwrap());
    (svc, tmp)
}

/// One corpus entry: a schema to define, fixture nodes to create, and the
/// request phrasings to resolve against them.
struct Case {
    label: &'static str,
    schema: Value,
    node_type: &'static str,
    fixtures: Vec<(&'static str, Value)>,
    requests: Vec<&'static str>,
}

/// The corpus deliberately over-samples the shapes the repairs exist for,
/// rather than sampling "typical" requests. A repair covers a malformation, so
/// the honest test of whether the gap is reachable is to push toward it: enum
/// fields with several legal values (the `in`-shaped pull), multi-value and
/// list-flavored phrasings, fields whose names invite quoting, and phrasings
/// mixing an identifier with an update target.
///
/// If the malformations still do not appear under this much pull, that is a
/// meaningfully stronger negative result than "they did not appear on ordinary
/// traffic".
fn corpus() -> Vec<Case> {
    vec![
        Case {
            label: "equipment/enum-multi-value",
            schema: json!({
                "name": "Equipment",
                "fields": [
                    {"name": "status", "type": "enum", "coreValues": [
                        {"value": "available", "label": "Available"},
                        {"value": "checked_out", "label": "Checked out"},
                        {"value": "returned", "label": "Returned"},
                        {"value": "in_repair", "label": "In repair"},
                        {"value": "retired", "label": "Retired"}
                    ]},
                    {"name": "replacement_cost", "type": "number"},
                    {"name": "serial", "type": "text"}
                ]
            }),
            node_type: "equipment",
            fixtures: vec![
                (
                    "Laser cutter",
                    json!({"status": "checked_out", "replacement_cost": 2400, "serial": "LC-88"}),
                ),
                (
                    "Band saw",
                    json!({"status": "in_repair", "replacement_cost": 900, "serial": "BS-12"}),
                ),
                (
                    "Drill press",
                    json!({"status": "available", "replacement_cost": 350, "serial": "DP-07"}),
                ),
            ],
            requests: vec![
                // Multi-value pull: two enum members named in one breath. This
                // is the phrasing most likely to reach for `in`, which the
                // decomposition prompt never instructs — #2185's stated bound.
                "Which gear is either checked out or in repair?",
                "Show me the items that are checked out, in repair, or retired",
                // Identifier-vs-update-target, the shape the prompt warns about.
                "The 2400 one came back — set it to returned",
                // Bare number identifier.
                "Find the one that costs 900",
                // Text identifier that looks like it wants quoting.
                "Pull up serial LC-88",
                // Nothing resolves to a typed field — should degrade to query.
                "Where's the thing we use for cutting sheet metal?",
            ],
        },
        Case {
            label: "invoice/date-and-number",
            schema: json!({
                "name": "Invoice",
                "fields": [
                    {"name": "amount", "type": "number"},
                    {"name": "due_date", "type": "date"},
                    {"name": "paid", "type": "boolean"},
                    {"name": "vendor", "type": "text"}
                ]
            }),
            node_type: "invoice",
            fixtures: vec![
                (
                    "Invoice #1",
                    json!({"amount": 500, "due_date": "2026-09-04", "paid": false,
                           "vendor": "Acme, Inc."}),
                ),
                (
                    "Invoice #2",
                    json!({"amount": 1200, "due_date": "2026-08-01", "paid": true,
                           "vendor": "Globex"}),
                ),
            ],
            requests: vec![
                "Mark the $500 invoice as paid",
                "Which invoice is due next Friday?",
                "Show me the overdue ones",
                // Boolean encoding: the shape `coerce_filter_value_to_field_type`
                // already handles, included so the harness reports whether that
                // coercion is still load-bearing.
                "Find the unpaid invoice",
                // A comma inside a legitimate stored value — the exact case
                // `split_in_operator_values` documents as its accepted tradeoff.
                // Worth knowing whether this path can even produce it.
                "The Acme, Inc. invoice needs a second look",
            ],
        },
    ]
}

/// How a single emitted filter object relates to the repairs that do not cover
/// this path. One per observed shape, plus the two catch-alls.
#[derive(Default, Debug)]
struct Tally {
    /// Filters that deserialized into `AgentFilterItem` cleanly.
    well_formed: usize,
    /// `deny_unknown_fields` rejects — the silent-narrowing hazard #2185 flags.
    /// These are dropped by production today.
    dropped_unknown_field: usize,
    /// A key wrapped in its own literal quote marks (`"\"property\""`).
    /// `repair_over_quoted_keys` covers this at the tool-call boundary.
    over_quoted_key: usize,
    /// A key carrying a leaked chat-template special token.
    /// `repair_leaked_special_token_keys` covers this at the tool-call boundary.
    leaked_token_key: usize,
    /// `operator: "in"` with a scalar (non-array) value — #2182's shape, which
    /// `repair_scalar_in_operator_values` covers at the tool-call boundary.
    scalar_in_value: usize,
    /// `operator: "in"` at all. Tracked separately because #2185's stated bound
    /// is that the decomposition prompt never instructs `in`; a nonzero count
    /// here refutes that bound regardless of the value's shape.
    used_in_operator: usize,
    /// A filter naming a property the schema does not define. Not a repair
    /// target — but it produces the same silent narrowing, so it is worth
    /// counting alongside.
    unknown_property: usize,
    /// Value's JSON type disagrees with the field's declared type, and
    /// `coerce_filter_value_to_field_type` fixed it. Confirms that coercion is
    /// still doing work.
    coerced_scalar_type: usize,
    /// Filter objects seen, total.
    total_filters: usize,
    /// Decompositions whose raw text did not yield parseable JSON at all.
    ///
    /// Counts what the MODEL emitted, deliberately — this classifies the raw
    /// dump, upstream of any repair production applies. So a nonzero count here
    /// does not mean resolution failed: `quote_bare_date_literals` recovers the
    /// bare-`YYYY-MM-DD` case that produces all of them today, and the printed
    /// per-request outcomes are where that recovery shows up. Read this as "how
    /// often the model emits invalid JSON", and the `resolved:` line as "how
    /// often that mattered".
    unparseable_output: usize,
    /// Decompositions that ran.
    total_decompositions: usize,
}

const SPECIAL_TOKEN_MARKERS: &[&str] = &[
    "<start_of_turn>",
    "<end_of_turn>",
    "<eos>",
    "<bos>",
    "<pad>",
    "<unused",
];

/// Classify one raw filter object from the decomposition output.
fn classify_filter(f: &Value, schema_fields: &[(String, String)], tally: &mut Tally) {
    tally.total_filters += 1;
    let Some(obj) = f.as_object() else {
        tally.dropped_unknown_field += 1;
        return;
    };

    for key in obj.keys() {
        if key.starts_with('"') && key.ends_with('"') && key.len() >= 2 {
            tally.over_quoted_key += 1;
        }
        if SPECIAL_TOKEN_MARKERS.iter().any(|m| key.contains(m)) {
            tally.leaked_token_key += 1;
        }
    }

    if obj.get("operator").and_then(Value::as_str) == Some("in") {
        tally.used_in_operator += 1;
        match obj.get("value") {
            Some(Value::Array(_)) => {}
            Some(_) => tally.scalar_in_value += 1,
            None => {}
        }
    }

    if let Some(prop) = obj.get("property").and_then(Value::as_str) {
        let declared = schema_fields.iter().find(|(n, _)| n == prop);
        match declared {
            None => tally.unknown_property += 1,
            Some((_, ty)) => {
                // Would `coerce_filter_value_to_field_type` change this value?
                let is_string_value = matches!(obj.get("value"), Some(Value::String(_)));
                if is_string_value && (ty == "number" || ty == "boolean") {
                    tally.coerced_scalar_type += 1;
                }
            }
        }
    }

    // The decisive check: does production's own deserialization accept it?
    match serde_json::from_value::<nodespace_core::ops::query_ops::AgentFilterItem>(f.clone()) {
        Ok(_) => tally.well_formed += 1,
        Err(_) => tally.dropped_unknown_field += 1,
    }
}

/// Read the dump file and return every `resolve_query` decomposition response
/// in it.
///
/// Identified by its prompt rather than by call order: the same dump file
/// carries every native-path call, and the decomposition prompt is uniquely
/// identifiable by its own opening sentence. Correlating on `seq` (which the
/// dump module emits for exactly this purpose) keeps the response tied to the
/// prompt that produced it.
fn decomposition_responses(dump: &PathBuf) -> Vec<String> {
    const DECOMP_MARKER: &str =
        "You resolve an ambiguous user request into a precise structured search query";

    let Ok(text) = std::fs::read_to_string(dump) else {
        return Vec::new();
    };

    let mut decomp_seqs: Vec<u64> = Vec::new();
    let mut responses: Vec<(u64, String)> = Vec::new();

    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let seq = v.get("seq").and_then(Value::as_u64).unwrap_or(u64::MAX);
        match v.get("kind").and_then(Value::as_str) {
            Some("prompt")
                if v.get("prompt")
                    .and_then(Value::as_str)
                    .is_some_and(|p| p.contains(DECOMP_MARKER)) =>
            {
                decomp_seqs.push(seq);
            }
            Some("response") => {
                if let Some(raw) = v.get("raw_response").and_then(Value::as_str) {
                    responses.push((seq, raw.to_string()));
                }
            }
            _ => {}
        }
    }

    responses
        .into_iter()
        .filter(|(seq, _)| decomp_seqs.contains(seq))
        .map(|(_, raw)| raw)
        .collect()
}

/// Drive the real `exec_resolve_query` across the corpus, capture every
/// decomposition's raw output, and print a shape tally.
///
/// Deliberately assertion-free about the model: this answers "what does this
/// path emit", and the answer is the printed tally. It does assert that the
/// harness actually reached its question — a run capturing zero decompositions
/// is a broken harness reporting a clean result, which is the failure mode
/// worth guarding.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "loads a ~5GB GGUF; run explicitly"]
async fn measure_decomposition_filter_shapes() {
    // `NODESPACE_DECOMP_DUMP_DIR` keeps the raw capture after the run for
    // inspection; without it the dump lands in a tempdir and is discarded.
    let keep_dir = std::env::var("NODESPACE_DECOMP_DUMP_DIR").ok();
    let dump_dir = TempDir::new().unwrap();
    let dump_path = match &keep_dir {
        Some(d) => PathBuf::from(d).join("decomp-dump.jsonl"),
        None => dump_dir.path().join("decomp-dump.jsonl"),
    };
    // Set before the engine loads so every generate() call is covered.
    std::env::set_var("NODESPACE_PROMPT_DUMP", &dump_path);

    let engine = tokio::task::spawn_blocking(load_engine)
        .await
        .expect("load task panicked");
    let engine: Arc<dyn ChatInferenceEngine> = Arc::new(engine);

    let mut tally = Tally::default();
    let mut resolved_count = 0usize;
    let mut outcomes: Vec<String> = Vec::new();
    let mut seen_responses = 0usize;

    for case in corpus() {
        let (ns, _tmp) = make_service().await;
        handle_create_schema(&ns, case.schema.clone())
            .await
            .expect("fixture schema must create");

        let executor = GraphToolExecutor {
            node_service: Some(ns.clone()),
            embedding_service: Arc::new(RwLock::new(None)),
            inference_engine: Some(engine.clone()),
            playbook_lifecycle: None,
        };

        for (title, props) in &case.fixtures {
            let created = executor
                .execute(
                    "create_node",
                    json!({
                        "content": title,
                        "node_type": case.node_type,
                        "field_values": props,
                    }),
                )
                .await
                .expect("fixture creation must succeed");
            assert!(!created.is_error, "fixture creation failed: {created:?}");
        }

        // The declared field list, used to tell an unknown property from a
        // known one and to predict which values coercion would rewrite.
        let schema_fields: Vec<(String, String)> = ns
            .get_schema_node(case.node_type)
            .await
            .expect("schema lookup must succeed")
            .map(|s| {
                s.fields
                    .iter()
                    .map(|f| (f.name.clone(), f.field_type.clone()))
                    .collect()
            })
            .unwrap_or_default();

        for request in &case.requests {
            for rep in 0..REPS {
                let before = decomposition_responses(&dump_path).len();

                let result = executor
                    .execute(
                        "resolve_query",
                        json!({ "request": request, "node_type": case.node_type }),
                    )
                    .await
                    .expect("resolve_query must not error the turn");

                let after = decomposition_responses(&dump_path);
                seen_responses = after.len();
                // Every decomposition emitted since the previous request.
                for raw in after.iter().skip(before) {
                    tally.total_decompositions += 1;
                    // Production's own helper, imported rather than
                    // reimplemented: this classification is only a claim about
                    // production if it extracts the same slice production does.
                    let slice = extract_json_object(raw).unwrap_or(raw);
                    match serde_json::from_str::<Value>(slice) {
                        Ok(parsed) => {
                            if let Some(filters) = parsed.get("filters").and_then(Value::as_array) {
                                for f in filters {
                                    classify_filter(f, &schema_fields, &mut tally);
                                }
                            }
                        }
                        Err(_) => tally.unparseable_output += 1,
                    }
                }

                if result.result["resolved"] == json!(true) {
                    resolved_count += 1;
                }
                // The raw decomposition text is the evidence this test exists
                // to produce — the tally is a summary of it, and a summary is
                // not auditable on its own.
                let raw = after
                    .iter()
                    .skip(before)
                    .map(|r| r.trim().to_string())
                    .collect::<Vec<_>>()
                    .join(" | ");
                outcomes.push(format!(
                    "  [{}] rep{rep} {:?}\n      decomposition: {raw}\n      -> {}",
                    case.label, request, result.result
                ));
            }
        }
    }

    std::env::remove_var("NODESPACE_PROMPT_DUMP");

    println!("\n=== resolve_query decomposition outcomes ===");
    for line in &outcomes {
        println!("{line}");
    }
    println!("\n=== filter shape tally ===");
    println!("{tally:#?}");
    println!("\nresolved: {resolved_count}/{} requests", outcomes.len());

    // Guard the harness, not the model. A run that captured nothing has not
    // answered #2185's question — and would otherwise print an all-zero tally
    // that reads exactly like "no malformations found".
    assert!(
        seen_responses > 0,
        "captured zero decomposition responses from the prompt dump — the harness did not \
         reach its question. Check that NODESPACE_PROMPT_DUMP is honored and that the \
         decomposition prompt marker still matches exec_resolve_query's prompt text."
    );
    assert!(
        tally.total_decompositions > 0,
        "no decompositions classified despite {seen_responses} captured responses"
    );
}
