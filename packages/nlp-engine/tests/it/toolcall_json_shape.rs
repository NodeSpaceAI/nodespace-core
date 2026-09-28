//! Live-model characterisation of malformed nested-object-array tool-call
//! arguments, against the real Gemma 4 E4B chat path.
//!
//! These record the measurement the fix rests on, so a later change to the chat
//! template, grammar, or model can be checked against the same evidence rather
//! than against a remembered claim:
//!
//! - A first attempt encodes `create_schema`'s array-of-objects `fields`
//!   cleanly (the tool-call grammar makes an over-quoted key unreachable).
//! - A retry copies whatever shape the prior assistant turn holds — malformed
//!   in, malformed out; clean in, clean out. That, not the model's encoding
//!   ability, is what made the reported retry loop unrecoverable.
//!
//! The repair itself lives in `agent_loop::repair_over_quoted_keys` and is
//! covered by fast unit tests there; these tests exist to keep the premise
//! honest, not to gate the fix.
//!
//! Ignored by default — each requires the E4B GGUF on disk and ~20s of GPU
//! time. Run explicitly with `--ignored --nocapture`.

#![cfg(feature = "chat-service")]

use nodespace_nlp_engine::{ChatChunk, ChatConfig, ChatEngine, ChatMessage, Role, ToolSpec};
use std::sync::{Arc, Mutex};

fn model_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    std::path::PathBuf::from(home).join(".nodespace/models/gemma-4-E4B-it-Q4_K_M.gguf")
}

/// A tool whose parameters are an array of objects — the reported malformed
/// shape. Mirrors `create_schema`'s `fields` closely enough that the model
/// faces the same encoding decision.
fn create_schema_tool() -> ToolSpec {
    ToolSpec {
        name: "create_schema".to_string(),
        description: "Create a new node type with typed fields.".to_string(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Name of the new node type"},
                "fields": {
                    "type": "array",
                    "description": "Field definitions for the new type",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string"},
                            "type": {"type": "string", "enum": ["text", "number", "date", "boolean"]}
                        },
                        "required": ["name", "type"]
                    }
                }
            },
            "required": ["name", "fields"]
        }),
    }
}

/// A tool with a flat (non-nested) parameter set, as a control. If the flat tool
/// encodes cleanly while the nested one does not, the fault is specific to
/// array-of-objects arguments rather than tool calling in general.
fn flat_tool() -> ToolSpec {
    ToolSpec {
        name: "search_nodes".to_string(),
        description: "Search for nodes by text query.".to_string(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        }),
    }
}

struct Captured {
    raw_pieces: String,
    tool_names: Vec<String>,
    args: String,
}

async fn run_turn(service: &ChatEngine, prompt: &str, tools: Vec<ToolSpec>) -> Captured {
    let raw = Arc::new(Mutex::new(String::new()));
    let names = Arc::new(Mutex::new(Vec::<String>::new()));
    let args = Arc::new(Mutex::new(String::new()));

    let (r, n, a) = (Arc::clone(&raw), Arc::clone(&names), Arc::clone(&args));
    service
        .generate_streaming(
            vec![ChatMessage::text(Role::User, prompt)],
            Some(tools),
            0.0,
            512,
            move |chunk| match chunk {
                ChatChunk::Token(t) => r.lock().unwrap().push_str(&t),
                ChatChunk::ToolCallStart { name, .. } => n.lock().unwrap().push(name),
                ChatChunk::ToolCallArgs { json, .. } => a.lock().unwrap().push_str(&json),
                _ => {}
            },
        )
        .await
        .expect("generation must succeed");

    let raw_pieces = raw.lock().unwrap().clone();
    let tool_names = names.lock().unwrap().clone();
    let args = args.lock().unwrap().clone();
    Captured {
        raw_pieces,
        tool_names,
        args,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the Gemma 4 E4B GGUF; run explicitly with --ignored --nocapture"]
async fn nested_object_array_arguments_encode_as_valid_json() {
    let path = model_path();
    if !path.exists() {
        eprintln!("SKIP: model not found at {}", path.display());
        return;
    }

    let service = ChatEngine::new(ChatConfig {
        n_ctx: 16384,
        n_gpu_layers: 99,
        // Q8_0 KV halves the cache footprint; at f16 a 16K window does not fit
        // alongside the weights on a 16 GB machine and the engine refuses to load.
        type_k: Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0),
        type_v: Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0),
        ..Default::default()
    })
    .expect("service construction");
    service
        .load_model(path.to_str().expect("model path is utf-8"), None)
        .expect("model load");

    // Repeat: the reported failure rate is ~3/8, so a single trial proves nothing.
    let trials = 8;
    let mut malformed = 0;
    for i in 0..trials {
        let cap = run_turn(
            &service,
            "Create a Venue node type with fields: capacity (number) and address (text).",
            vec![create_schema_tool()],
        )
        .await;

        let joined = cap.args.clone();
        println!("--- trial {i} ---");
        println!("tools: {:?}", cap.tool_names);
        println!("args:  {joined}");
        println!("raw text: {:?}", cap.raw_pieces);

        if joined.is_empty() {
            continue;
        }
        let parsed: serde_json::Value = match serde_json::from_str(&joined) {
            Ok(v) => v,
            Err(e) => {
                println!("  -> args are not valid JSON: {e}");
                malformed += 1;
                continue;
            }
        };
        // The reported malformation: keys that literally include quote marks.
        if let Some(fields) = parsed.get("fields").and_then(|f| f.as_array()) {
            for f in fields {
                if let Some(obj) = f.as_object() {
                    if obj.keys().any(|k| k.contains('"')) {
                        println!("  -> MALFORMED KEYS: {:?}", obj.keys().collect::<Vec<_>>());
                        malformed += 1;
                    }
                }
            }
        }
    }

    println!("malformed trials: {malformed}/{trials}");
    assert_eq!(
        malformed, 0,
        "a first attempt must encode nested-object-array arguments cleanly; \
         the tool-call grammar should make an over-quoted key unreachable"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the Gemma 4 E4B GGUF; run explicitly with --ignored --nocapture"]
async fn flat_arguments_control() {
    let path = model_path();
    if !path.exists() {
        eprintln!("SKIP: model not found at {}", path.display());
        return;
    }

    let service = ChatEngine::new(ChatConfig {
        n_ctx: 16384,
        n_gpu_layers: 99,
        // Q8_0 KV halves the cache footprint; at f16 a 16K window does not fit
        // alongside the weights on a 16 GB machine and the engine refuses to load.
        type_k: Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0),
        type_v: Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0),
        ..Default::default()
    })
    .expect("service construction");
    service
        .load_model(path.to_str().expect("model path is utf-8"), None)
        .expect("model load");

    for i in 0..4 {
        let cap = run_turn(&service, "Find nodes about billing.", vec![flat_tool()]).await;
        println!(
            "--- flat trial {i} --- tools={:?} args={}",
            cap.tool_names, cap.args
        );
    }
}

/// The reported scenario, not the happy path: a first `create_schema` call was
/// rejected, its error came back as a tool result, and the model is asked to
/// retry. This replays a prior assistant tool-call turn through
/// `chat_message_to_oai_value` — the one place a well-formed argument string is
/// re-serialized into the template — which the single-turn test never exercises.
///
/// Runs both arms in one pass. Holding everything but the prior turn's shape
/// fixed is the whole experiment: it is what separates "the model cannot encode
/// nested arguments" (refuted — the control arm retries cleanly) from "the model
/// copies whatever shape it reads back out of its own history" (what the
/// malformed arm shows, and what the repair exists to neutralise). Asserting
/// both here means neither arm can rot unnoticed, and reading the control no
/// longer requires making the test fail on purpose.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the Gemma 4 E4B GGUF; run explicitly with --ignored --nocapture"]
async fn retry_copies_the_shape_of_the_prior_call() {
    let path = model_path();
    if !path.exists() {
        eprintln!("SKIP: model not found at {}", path.display());
        return;
    }

    let service = ChatEngine::new(ChatConfig {
        n_ctx: 16384,
        n_gpu_layers: 99,
        type_k: Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0),
        type_v: Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0),
        ..Default::default()
    })
    .expect("service construction");
    service
        .load_model(path.to_str().expect("model path is utf-8"), None)
        .expect("model load");

    let malformed_prior =
        r#"{"name":"Venue","fields":[{"\"name\"":"capacity","\"type\"":"number"}]}"#;
    let clean_prior = r#"{"name":"Venue","fields":[{"name":"capacity","type":"number"}]}"#;

    let trials = 8;
    for (arm, prior_args, expect_malformed_trials) in [
        ("malformed-prior", malformed_prior, trials),
        ("clean-prior", clean_prior, 0),
    ] {
        // Counted per trial, not per malformed field. Fusing the two would tie the
        // assertion to how many fields the model happens to emit, so a run with the
        // same behaviour but a different field count would fail while naming the
        // wrong conclusion — and could prompt removing a repair that is still needed.
        let mut malformed_trials = 0;
        let mut malformed_fields = 0;
        for i in 0..trials {
            let messages = vec![
            ChatMessage::text(
                Role::User,
                "Create a Venue node type with fields: capacity (number) and address (text).",
            ),
            ChatMessage {
                role: Role::Assistant,
                content: String::new(),
                tool_calls: vec![nodespace_nlp_engine::ToolCallRaw {
                    id: "call_1".to_string(),
                    function_name: "create_schema".to_string(),
                    arguments_json: prior_args.to_string(),
                    provider_extra: None,
                }],
                tool_call_id: None,
                name: None,
                reasoning: None,
            },
            ChatMessage {
                role: Role::Tool,
                content: "error: invalid params: fields[0] is missing \"name\"; fields[0] is                           missing \"type\". Every entry in \"fields\" needs both \"name\" and                           \"type\", e.g. {\"name\":\"amount\",\"type\":\"number\"}. Re-send the                           call with only the listed entries corrected — leave every other field                           exactly as it was."
                    .to_string(),
                tool_calls: Vec::new(),
                tool_call_id: Some("call_1".to_string()),
                name: Some("create_schema".to_string()),
                reasoning: None,
            },
        ];

            let raw = Arc::new(Mutex::new(String::new()));
            let names = Arc::new(Mutex::new(Vec::<String>::new()));
            let args = Arc::new(Mutex::new(String::new()));
            let (r, n, a) = (Arc::clone(&raw), Arc::clone(&names), Arc::clone(&args));
            service
                .generate_streaming(
                    messages,
                    Some(vec![create_schema_tool()]),
                    0.0,
                    512,
                    move |chunk| match chunk {
                        ChatChunk::Token(t) => r.lock().unwrap().push_str(&t),
                        ChatChunk::ToolCallStart { name, .. } => n.lock().unwrap().push(name),
                        ChatChunk::ToolCallArgs { json, .. } => a.lock().unwrap().push_str(&json),
                        _ => {}
                    },
                )
                .await
                .expect("generation must succeed");

            let joined = args.lock().unwrap().clone();
            let text = raw.lock().unwrap().clone();
            let tools_called = names.lock().unwrap().clone();
            println!("--- retry trial {i} ---");
            println!("tools: {tools_called:?}");
            println!("args:  {joined}");
            println!("text:  {text:?}");

            if joined.is_empty() {
                println!("  -> NO TOOL CALL (turn produced text only)");
                continue;
            }
            let trial_malformed_fields = match serde_json::from_str::<serde_json::Value>(&joined) {
                Ok(parsed) => {
                    let mut count = 0;
                    if let Some(fields) = parsed.get("fields").and_then(|f| f.as_array()) {
                        for f in fields {
                            if let Some(obj) = f.as_object() {
                                if obj.keys().any(|k| k.contains('"')) {
                                    println!(
                                        "  -> MALFORMED KEYS: {:?}",
                                        obj.keys().collect::<Vec<_>>()
                                    );
                                    count += 1;
                                }
                            }
                        }
                    }
                    count
                }
                Err(e) => {
                    // Unparseable arguments are a different failure than over-quoted
                    // keys, but they are equally not a clean retry, so the trial counts.
                    println!("  -> args are not valid JSON: {e}");
                    1
                }
            };
            if trial_malformed_fields > 0 {
                malformed_trials += 1;
                malformed_fields += trial_malformed_fields;
            }
        }
        println!(
            "[{arm}] malformed trials: {malformed_trials}/{trials} \
         ({malformed_fields} malformed fields total)"
        );
        // Asserted as the *measured* behaviour, not as desirable behaviour. The
        // malformed arm records that the model copies the bad shape out of its own
        // history every time — the premise `repair_over_quoted_keys` neutralises —
        // and the clean arm records that the very same prompt retries cleanly when
        // only that shape differs. If either stops holding, the diagnosis behind the
        // repair needs re-examining before the repair itself is touched.
        assert_eq!(
            malformed_trials, expect_malformed_trials,
            "[{arm}] retry shape must follow the prior call's shape; if this no longer \
         holds, re-check whether agent_loop::repair_over_quoted_keys is still \
         load-bearing"
        );
    }
}
