//! A small local model creating a record from a markdown document through the
//! agent's `create_node` tool: the first line is the title, the rest becomes
//! child nodes (the same split `nodespace node create` makes).
//!
//! One turn on the real production tool definitions (`create_node`,
//! `create_nodes_from_markdown`, `create_task`) over a real store. The
//! assertion is on what the STORE holds afterwards, never on what the tool
//! returned: the root's type, its title (the first line without the heading
//! marker, on one line), the types of the child nodes it was split into and
//! how many nodes sit below it.
//!
//! Four scenarios, `REPS` repetitions each, because a single run of a small
//! model is not decision-grade:
//!   - a task given as a document with a checklist (the model may use
//!     `create_task`, which makes a task or an issue),
//!   - the same task with `create_task` withheld, so the document form of
//!     `create_node` is the only route,
//!   - a decision given as a document with its reasoning,
//!   - a spec given as a document with three sections.
//!
//! The run passes when every scenario creates the right record in at least
//! `MIN_PASSES` of its `REPS` repetitions; each repetition is scored on the
//! root's type and title, the child types and the nodes below it.
//!
//! The run is ignored by default and needs the CPU-only environment of
//! `live_small`:
//! ```text
//! NODESPACE_FORCE_CPU=1 NODESPACE_LIVE_THREADS=4 \
//! NODESPACE_LIVE_CHAT_MODEL=<scratch>/models/<chat>.gguf \
//! cargo test -p nodespace-agent --test it live_create_node_document:: -- --ignored --nocapture --test-threads=1
//! ```

use std::sync::Arc;

use nodespace_agent::local_agent::tools::{all_tool_definitions, GraphToolExecutor};
use nodespace_agent::AgentToolExecutor;
use nodespace_core::db::SqliteStore;
use nodespace_core::services::NodeService;
use nodespace_nlp_engine::inference::local::LlamaChatInferenceEngine;
use nodespace_nlp_engine::{
    ChatInferenceEngine, ChatMessage, InferenceRequest, Role, StreamingChunk, ToolDefinition,
};
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::RwLock;

use crate::live_small::{load_engine, require_cpu_only};

const REPS: usize = 3;
/// How many of the `REPS` repetitions of every scenario must create the right
/// record: a small model on a CPU is not deterministic, so one miss is allowed.
const MIN_PASSES: usize = 2;

/// The system prompt carries the two things the production loop always
/// supplies for a create: that the user's wording names no existing record,
/// and that the model acts without asking.
const SYSTEM: &str = "You are a graph-editing assistant. Act on the user's request immediately \
    using the tools available. Do not ask the user to confirm.\n\
    MENTIONED ENTITIES: none found\n\
    EXISTING SCHEMAS: none beyond the built-in record types (text, task, spec, plan, decision).";

struct Scenario {
    name: &'static str,
    user: &'static str,
    /// The tools the model is offered.
    tools: &'static [&'static str],
    /// The types the record may have (`create_task` makes a task or an issue).
    node_types: &'static [&'static str],
    /// The root's title: the first line of the document, heading marker removed.
    title: &'static str,
    /// The types the root's direct children must include (each listed type as
    /// many times as it appears here).
    child_types: &'static [&'static str],
    /// The least number of nodes below the root that the rest of the document becomes.
    min_children: usize,
}

const ALL_CREATES: &[&str] = &["create_node", "create_nodes_from_markdown", "create_task"];
const WITHOUT_CREATE_TASK: &[&str] = &["create_node", "create_nodes_from_markdown"];

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "task",
        tools: ALL_CREATES,
        user: "Create a task from this document:\n\n\
               # Fix the login redirect loop\n\n\
               Users who open a deep link while signed out are sent back to the login page forever.\n\n\
               1. Reproduce it with a deep link\n\
               2. Keep the target address through the sign-in\n\
               3. Add a regression test",
        node_types: &["task", "issue"],
        title: "Fix the login redirect loop",
        child_types: &["text"],
        min_children: 2,
    },
    Scenario {
        name: "task, create_task withheld",
        tools: WITHOUT_CREATE_TASK,
        user: "Create a task from this document:\n\n\
               # Fix the login redirect loop\n\n\
               Users who open a deep link while signed out are sent back to the login page forever.\n\n\
               1. Reproduce it with a deep link\n\
               2. Keep the target address through the sign-in\n\
               3. Add a regression test",
        node_types: &["task"],
        title: "Fix the login redirect loop",
        child_types: &["text"],
        min_children: 2,
    },
    Scenario {
        name: "decision",
        tools: ALL_CREATES,
        user: "Record this decision as a decision record:\n\n\
               # Use SQLite for the local store\n\n\
               We keep the local store on SQLite.\n\n\
               ## Why\n\n\
               It is one file, it needs no server, and every platform we ship on already has it.\n\n\
               ## Alternatives\n\n\
               A separate database server was rejected because it adds an install step.",
        node_types: &["decision"],
        title: "Use SQLite for the local store",
        child_types: &["text", "header", "header"],
        min_children: 3,
    },
    Scenario {
        name: "spec with three sections",
        tools: ALL_CREATES,
        user: "Create a spec from this document:\n\n\
               # Offline mode for the mobile app\n\n\
               ## Goal\n\n\
               The app keeps working without a connection.\n\n\
               ## Scope\n\n\
               Reading and editing notes. Search over what is already on the device.\n\n\
               ## Non-goals\n\n\
               Syncing the changes while offline.",
        node_types: &["spec"],
        title: "Offline mode for the mobile app",
        child_types: &["header", "header", "header"],
        min_children: 3,
    },
];

/// Whether `children` holds every type of `wanted`, each as many times as it
/// is listed there.
fn holds_child_types(children: &[String], wanted: &[&str]) -> bool {
    let mut left: Vec<&str> = children.iter().map(String::as_str).collect();
    wanted
        .iter()
        .all(|w| match left.iter().position(|c| c == w) {
            Some(at) => {
                left.swap_remove(at);
                true
            }
            None => false,
        })
}

fn write_tools(names: &[&str]) -> Vec<ToolDefinition> {
    all_tool_definitions()
        .into_iter()
        .filter(|t| names.contains(&t.name.as_str()))
        .collect()
}

async fn make_executor() -> (GraphToolExecutor, Arc<NodeService>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let mut store: Arc<SqliteStore> =
        Arc::new(SqliteStore::new(tmp.path().join("live.db")).await.unwrap());
    let ns = Arc::new(NodeService::new(&mut store).await.unwrap());
    let executor = GraphToolExecutor {
        node_service: Some(ns.clone()),
        embedding_service: Arc::new(RwLock::new(None)),
        inference_engine: None,
        playbook_lifecycle: None,
    };
    (executor, ns, tmp)
}

/// One turn: the first tool call the model makes, as (name, arguments).
async fn first_call(
    engine: &LlamaChatInferenceEngine,
    tools: &[&str],
    user: &str,
) -> (Option<(String, String)>, String) {
    let request = InferenceRequest {
        messages: vec![
            ChatMessage::text(Role::System, SYSTEM.to_string()),
            ChatMessage::text(Role::User, user.to_string()),
        ],
        tools: Some(write_tools(tools)),
        temperature: Some(0.1),
        max_tokens: Some(1024),
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

/// What the store holds for the record the call created: the root's type and
/// title, and the types of its children.
struct Stored {
    node_type: String,
    title: String,
    /// The types of the root's direct children.
    children: Vec<String>,
    /// How many nodes sit anywhere below the root: a paragraph followed by a
    /// list nests the list items under the paragraph, so the direct children
    /// alone undercount what the document became.
    descendants: usize,
}

async fn stored_root(ns: &NodeService, result: &Value) -> Option<Stored> {
    let id = result
        .get("id")
        .and_then(Value::as_str)?
        .trim_start_matches("nodespace://")
        .to_string();
    let node = ns.get_node(&id).await.ok()??;
    let children = ns.get_children(&id).await.ok()?;
    let mut descendants = 0usize;
    let mut pending: Vec<String> = children.iter().map(|c| c.id.clone()).collect();
    while let Some(next) = pending.pop() {
        descendants += 1;
        for child in ns.get_children(&next).await.ok()? {
            pending.push(child.id.clone());
        }
    }
    Some(Stored {
        node_type: node.node_type.clone(),
        title: node.content.clone(),
        children: children.iter().map(|c| c.node_type.clone()).collect(),
        descendants,
    })
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the locked native GGUF on disk"]
async fn a_document_given_to_create_node_becomes_a_titled_record_with_children() {
    require_cpu_only();

    // What the model is shown for the tool, as the engine receives it.
    let create = write_tools(ALL_CREATES)
        .into_iter()
        .find(|t| t.name == "create_node")
        .expect("create_node is a tool");
    println!(
        "PROMPT DUMP create_node description:\n{}\n",
        create.description
    );
    println!(
        "PROMPT DUMP create_node content parameter:\n{}\n",
        create.parameters_schema["properties"]["content"]["description"]
    );

    let engine = load_engine();
    let mut passed = vec![0usize; SCENARIOS.len()];

    for (index, scenario) in SCENARIOS.iter().enumerate() {
        for rep in 0..REPS {
            let (executor, ns, _tmp) = make_executor().await;
            let (call, raw) = first_call(&engine, scenario.tools, scenario.user).await;
            let Some((name, args_json)) = call else {
                println!(
                    "LIVE[{} rep {rep}] FAIL no tool call; text: {raw:?}",
                    scenario.name
                );
                continue;
            };
            println!("LIVE[{} rep {rep}] call {name}({args_json})", scenario.name);
            let Ok(args) = serde_json::from_str::<Value>(&args_json) else {
                println!(
                    "LIVE[{} rep {rep}] FAIL unparseable arguments",
                    scenario.name
                );
                continue;
            };
            let outcome = executor.execute(&name, args).await;
            let result = match outcome {
                Ok(r) if !r.is_error => r.result,
                Ok(r) => {
                    println!(
                        "LIVE[{} rep {rep}] FAIL tool error: {}",
                        scenario.name, r.result
                    );
                    continue;
                }
                Err(e) => {
                    println!("LIVE[{} rep {rep}] FAIL rejected: {e}", scenario.name);
                    continue;
                }
            };
            let Some(stored) = stored_root(&ns, &result).await else {
                println!(
                    "LIVE[{} rep {rep}] FAIL no single record id in the result: {result}",
                    scenario.name
                );
                continue;
            };
            let right_type = scenario.node_types.contains(&stored.node_type.as_str());
            let right_title = stored.title.trim() == scenario.title;
            let right_child_types = holds_child_types(&stored.children, scenario.child_types);
            let enough_children = stored.descendants >= scenario.min_children;
            println!(
                "LIVE[{} rep {rep}] stored type={} title={:?} children={:?} descendants={} -> type {} title {} child types {} nodes below {}",
                scenario.name,
                stored.node_type,
                stored.title,
                stored.children,
                stored.descendants,
                right_type,
                right_title,
                right_child_types,
                enough_children
            );
            if right_type && right_title && right_child_types && enough_children {
                passed[index] += 1;
            }
        }
    }

    for (index, scenario) in SCENARIOS.iter().enumerate() {
        println!(
            "LIVE SUMMARY {}: {}/{REPS} created the right record",
            scenario.name, passed[index]
        );
    }
    for (index, scenario) in SCENARIOS.iter().enumerate() {
        assert!(
            passed[index] >= MIN_PASSES,
            "{}: only {}/{REPS} repetitions created the right record, at least {MIN_PASSES} are required",
            scenario.name,
            passed[index]
        );
    }
}
