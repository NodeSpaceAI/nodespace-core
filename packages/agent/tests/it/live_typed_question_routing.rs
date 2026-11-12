//! A question typed in a type chat takes the read-only lookup route, on a
//! small local model.
//!
//! The chat is the one the app opens on the built-in Software Project type:
//! the skill that changes types is pinned, the type is the subject, the
//! opening message is the description the daemon renders from the stored
//! definition, and the four change options plus "Explain this type" are
//! offered. Two questions are then typed. Each must
//!
//!   - skip Stage 1 (no route decision is requested from the model),
//!   - be offered a read-only tool set (no write tool is declared to the
//!     model in any generation of the turn), and
//!   - be answered from the stored definition without contradicting the
//!     opening description.
//!
//! The route and the tool set are asserted from the prompts the model was
//! sent (`NODESPACE_PROMPT_DUMP`) and from the executed calls, not from the
//! wording of the answer, which depends on the model. The answers are
//! checked only for naming the parent type and for not denying it.
//!
//! Ignored by default: it loads two GGUF files. See `live_small` for the
//! environment that keeps it on the CPU.
//! ```text
//! NODESPACE_FORCE_CPU=1 NODESPACE_LIVE_THREADS=4 \
//! NODESPACE_LIVE_CHAT_MODEL=<scratch>/models/<chat>.gguf \
//! NODESPACE_LIVE_EMBED_MODEL=<scratch>/models/<embed>.gguf \
//! NODESPACE_PROMPT_DUMP=<scratch>/typed-question.jsonl \
//! cargo nextest run -p nodespace-agent --test it live_typed_question_routing:: \
//!   --run-ignored only -j 1 --no-capture
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use nodespace_agent::agent_types::PinnedSubject;
use nodespace_agent::local_agent::agent_loop::LocalAgentService;
use nodespace_agent::local_agent::tools::{pinned_skill_candidates, GraphToolExecutor};
use nodespace_agent::prompt_assembler::PromptAssembler;
use nodespace_agent::skill_pipeline::{seed_skill_nodes, SCHEMA_CHANGING_SKILL_ID};
use nodespace_core::db::SqliteStore;
use nodespace_core::markdown::prepare_nodes_from_template;
use nodespace_core::ops::context_ops::build_workspace_context;
use nodespace_core::schema::description::{describe_schema, SchemaDescription};
use nodespace_core::services::node_service::CreateNodeParams;
use nodespace_core::services::{
    InsertPositionOwned, NodeAccessor, NodeEmbeddingService, NodeService,
};
use nodespace_nlp_engine::chat::{ChatMessage, Role};
use nodespace_nlp_engine::inference::local::LlamaChatInferenceEngine;
use nodespace_nlp_engine::EmbeddingService;
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::RwLock;

use super::live_small::{load_embedder, load_engine};

const TYPE_ID: &str = "software-project";
const TYPE_TITLE: &str = "Software Project";

const OPTIONS: [&str; 5] = [
    "Explain this type",
    "Add a field",
    "Change a field",
    "Add a value to a list",
    "Remove a field",
];

/// Every tool the type-changing skill, or any other skill, can write with.
/// A lookup turn declares none of them to the model.
const WRITE_TOOLS: [&str; 7] = [
    "update_schema",
    "create_schema",
    "create_node",
    "update_node",
    "delete_node",
    "create_relationship",
    "delete_relationship",
];

struct Fixture {
    node_service: Arc<NodeService>,
    embedding_service: Arc<NodeEmbeddingService>,
    executor: Arc<GraphToolExecutor>,
    _tmp: TempDir,
}

async fn fixture(
    engine: Arc<LlamaChatInferenceEngine>,
    embedder: Arc<EmbeddingService>,
) -> Fixture {
    let tmp = TempDir::new().expect("tempdir");
    let mut store = Arc::new(
        SqliteStore::new(tmp.path().join("live.db"))
            .await
            .expect("store must open"),
    );
    let node_service = Arc::new(NodeService::new(&mut store).await.expect("node service"));
    let node_accessor: Arc<dyn NodeAccessor> = node_service.clone();
    let embedding_service = Arc::new(NodeEmbeddingService::new(
        embedder,
        store.clone(),
        node_accessor,
        node_service.behaviors().clone(),
    ));

    for tmpl in seed_skill_nodes() {
        let prepared = prepare_nodes_from_template(&tmpl).expect("template must parse");
        for p in &prepared {
            node_service
                .create_node_with_parent(CreateNodeParams {
                    id: Some(p.id.clone()),
                    node_type: p.node_type.clone(),
                    content: p.content.clone(),
                    parent_id: p.parent_id.clone(),
                    position: InsertPositionOwned::End,
                    properties: p.properties.clone(),
                    lifecycle_status: None,
                })
                .await
                .expect("skill node must insert");
        }
        embedding_service
            .embed_root_node(&prepared[0].id)
            .await
            .expect("skill root must embed");
    }

    let executor = Arc::new(GraphToolExecutor {
        node_service: Some(node_service.clone()),
        embedding_service: Arc::new(RwLock::new(Some(embedding_service.clone()))),
        inference_engine: Some(engine),
        playbook_lifecycle: None,
    });
    Fixture {
        node_service,
        embedding_service,
        executor,
        _tmp: tmp,
    }
}

/// The opening message the app puts in a type chat: the description rendered
/// from the stored definition, as the daemon does it.
async fn opening_message(node_service: &NodeService) -> String {
    let schema = node_service
        .get_schema_node(TYPE_ID)
        .await
        .expect("schema read")
        .expect("software-project is built in");
    let (fields, owners, _chain) = node_service
        .resolve_field_owners(TYPE_ID)
        .await
        .expect("fields resolve");
    let purpose = node_service
        .get_children(TYPE_ID)
        .await
        .unwrap_or_default()
        .iter()
        .map(|child| child.content.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut names: HashMap<String, String> = HashMap::new();
    for id in schema.extends.iter().chain(owners.values()) {
        if names.contains_key(id) {
            continue;
        }
        if let Ok(Some(named)) = node_service.get_schema_node(id).await {
            names.insert(id.clone(), named.envelope.content);
        }
    }
    describe_schema(
        &format!("[{TYPE_TITLE}](nodespace://{TYPE_ID})"),
        &SchemaDescription {
            schema: &schema,
            purpose: Some(purpose.as_str()),
            fields: &fields,
            owners: &owners,
            names: &names,
        },
    )
}

/// What one typed question did.
struct Turn {
    calls: Vec<String>,
    reply: String,
    /// The rendered prompt of each generation the turn made, in order.
    prompts: Vec<String>,
}

/// The records of the dump written after `from_line`.
fn dumped_prompts(path: &str, from_line: usize) -> (Vec<String>, usize) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let prompts = lines[from_line.min(lines.len())..]
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["kind"] == "prompt" && v["engine"] == "native")
        .filter_map(|v| v["sent"].as_str().map(str::to_string))
        .collect();
    (prompts, lines.len())
}

async fn typed_question(
    engine: Arc<LlamaChatInferenceEngine>,
    fx: &Fixture,
    opening: &str,
    message: &str,
    dump_path: &str,
) -> Turn {
    let before = dumped_prompts(dump_path, 0).1;
    let service = LocalAgentService::new_with_assembler(
        engine,
        fx.executor.clone(),
        Some(Arc::new(PromptAssembler::new(fx.node_service.clone()))),
    );
    let session = service
        .create_session(None, vec![ChatMessage::text(Role::Assistant, opening)])
        .await;
    let ctx = build_workspace_context(
        &fx.node_service,
        Some(&fx.embedding_service),
        Some(message),
        Some(message),
    )
    .await
    .expect("workspace context must build");
    service
        .set_session_context(&session, ctx.format_for_prompt(4000))
        .await;
    let skills = pinned_skill_candidates(&fx.node_service, &[SCHEMA_CHANGING_SKILL_ID.to_string()])
        .await
        .expect("the pinned skill must load");
    service
        .set_session_pins(
            &session,
            skills,
            vec![PinnedSubject {
                title: Some(TYPE_TITLE.to_string()),
                node_type: "schema".to_string(),
            }],
        )
        .await;
    service
        .set_session_offered_options(&session, OPTIONS.iter().map(|o| o.to_string()).collect())
        .await;

    let result = service
        .send_message(&session, message, |_| {}, |_| {})
        .await
        .expect("turn must complete");
    let calls = service
        .get_session(&session)
        .await
        .expect("session must survive the turn")
        .tool_executions
        .iter()
        .map(|t| t.name.clone())
        .collect();
    let (prompts, _) = dumped_prompts(dump_path, before);
    Turn {
        calls,
        reply: result.response,
        prompts,
    }
}

/// The Stage-1 decision tool; a generation that declares it is a route
/// decision, whatever else its prose says.
const STAGE_1_TOOL: &str = "route_query";

/// The read tool the pinned type skill keeps on the lookup route.
const LOOKUP_READ_TOOL: &str = "get_node";

/// Whether a rendered prompt declares `tool` to the model. The one place the
/// declaration forms of the chat template are written down: the write-tool
/// check, the Stage-1 check and the self-check below all go through it.
fn declares(prompt: &str, tool: &str) -> bool {
    prompt.contains(&format!("declaration:{tool}"))
        || prompt.contains(&format!("\"name\": \"{tool}\""))
        || prompt.contains(&format!("\"name\":\"{tool}\""))
}

fn declared_tools(prompt: &str) -> Vec<&'static str> {
    WRITE_TOOLS
        .iter()
        .copied()
        .filter(|tool| declares(prompt, tool))
        .collect()
}

/// Checks the matcher against the prompt it is reading, so a template whose
/// declaration form it does not know fails loudly instead of passing as
/// "no write tool declared". Every lookup generation must declare the read
/// tool the route keeps.
fn matcher_sees_the_tools(question: &str, i: usize, prompt: &str) -> Option<String> {
    (!declares(prompt, LOOKUP_READ_TOOL)).then(|| {
        format!(
            "{question}: generation {i} does not declare {LOOKUP_READ_TOOL}: either the \
             lookup route dropped it or the declaration matcher does not know this \
             template's form (read the dump)"
        )
    })
}

fn check_lookup_route(question: &str, turn: &Turn) -> Vec<String> {
    let mut failures = Vec::new();
    if turn.prompts.is_empty() {
        failures.push(format!("{question}: no prompt was captured"));
    }
    for (i, prompt) in turn.prompts.iter().enumerate() {
        failures.extend(matcher_sees_the_tools(question, i, prompt));
        if declares(prompt, STAGE_1_TOOL) {
            failures.push(format!(
                "{question}: generation {i} is a Stage-1 route decision"
            ));
        }
        let writes = declared_tools(prompt);
        if !writes.is_empty() {
            failures.push(format!(
                "{question}: generation {i} declares write tools {writes:?}"
            ));
        }
    }
    if let Some(write) = turn
        .calls
        .iter()
        .find(|c| WRITE_TOOLS.contains(&c.as_str()))
    {
        failures.push(format!("{question}: executed the write tool {write}"));
    }
    failures
}

/// Whether a reply says the type extends `Project`: the parent's name next to
/// extends or parent wording, with the type's own name removed first so
/// "Software Project" does not count as the parent. A denial is a reply that
/// says so in the common phrasings. Phrase lists can be evaded by paraphrase,
/// so a pass here is a weak signal and a failure is a prompt to read the reply.
fn names_the_parent(reply: &str) -> bool {
    let reply = reply.to_lowercase().replace("software project", "");
    ["extends", "parent", "inherits", "based on", "built on"]
        .iter()
        .any(|w| {
            reply.match_indices(w).any(|(at, _)| {
                let near: String = reply[at..].chars().take(60).collect();
                near.contains("project")
            })
        })
}

fn denies_the_parent(reply: &str) -> bool {
    let reply = reply.to_lowercase();
    [
        "does not extend",
        "doesn't extend",
        "extends nothing",
        "no parent",
        "not extend any",
        "does not inherit",
        "doesn't inherit",
        "standalone",
    ]
    .iter()
    .any(|p| reply.contains(p))
}

#[test]
fn the_parent_check_ignores_the_types_own_name() {
    assert!(!names_the_parent(
        "The Software Project type has a repository field."
    ));
    assert!(names_the_parent("Yes, Software Project extends Project."));
    assert!(names_the_parent(
        "Its parent type is [Project](nodespace://project)."
    ));
    assert!(!names_the_parent("No other type."));
    assert!(denies_the_parent("It extends nothing."));
    assert!(!denies_the_parent("Yes, it extends Project."));
}

#[test]
fn the_matcher_reads_the_declaration_forms_and_ignores_prose() {
    let declared = "<|tool|>declaration:get_node{} declaration:update_schema{}";
    assert!(declares(declared, "get_node"));
    assert_eq!(declared_tools(declared), ["update_schema"]);
    let prose = "Stage 1 decides with route_query or route_clarify; use update_schema later.";
    assert!(!declares(prose, STAGE_1_TOOL));
    assert!(declared_tools(prose).is_empty());
    assert!(matcher_sees_the_tools("q", 0, prose).is_some());
    assert!(matcher_sees_the_tools("q", 0, declared).is_none());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "loads a chat model and the embedding model"]
async fn typed_questions_in_the_software_project_chat_take_the_read_only_lookup_route() {
    let dump_dir = TempDir::new().unwrap();
    let dump_path = std::env::var("NODESPACE_PROMPT_DUMP")
        .unwrap_or_else(|_| dump_dir.path().join("dump.jsonl").display().to_string());
    std::env::set_var("NODESPACE_PROMPT_DUMP", &dump_path);

    let engine = tokio::task::spawn_blocking(load_engine).await.unwrap();
    let embedder = tokio::task::spawn_blocking(load_embedder).await.unwrap();
    let fx = fixture(engine.clone(), embedder).await;
    fx.embedding_service
        .embed_root_node(TYPE_ID)
        .await
        .expect("software-project schema must embed");
    let opening = opening_message(&fx.node_service).await;
    eprintln!("OPENING MESSAGE:\n{opening}\n");
    assert!(
        opening.contains("extends [Project]"),
        "the opening names the parent: {opening}"
    );

    let mut failures = Vec::new();

    let q1 = "Does this type extend another type?";
    let t1 = typed_question(engine.clone(), &fx, &opening, q1, &dump_path).await;
    eprintln!(
        "Q1 {q1:?}\n  generations={} calls={:?}\n  REPLY: {}\n",
        t1.prompts.len(),
        t1.calls,
        t1.reply
    );
    failures.extend(check_lookup_route(q1, &t1));
    if !names_the_parent(&t1.reply) {
        failures.push(format!(
            "{q1}: the reply does not name Project as the parent"
        ));
    }
    if denies_the_parent(&t1.reply) {
        failures.push(format!("{q1}: the reply denies the parent"));
    }

    let q2 = "Explain this type";
    let t2 = typed_question(engine.clone(), &fx, &opening, q2, &dump_path).await;
    eprintln!(
        "Q2 {q2:?}\n  generations={} calls={:?}\n  REPLY: {}\n",
        t2.prompts.len(),
        t2.calls,
        t2.reply
    );
    failures.extend(check_lookup_route(q2, &t2));
    let r2 = t2.reply.to_lowercase();
    if denies_the_parent(&t2.reply) {
        failures.push(format!(
            "{q2}: the reply contradicts the opening: it denies the parent"
        ));
    }
    if !r2.contains("repository") {
        failures.push(format!(
            "{q2}: the reply leaves out the type's own field, repository"
        ));
    }

    assert!(failures.is_empty(), "{failures:#?}");
}
