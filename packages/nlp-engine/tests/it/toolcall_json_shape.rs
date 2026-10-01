//! Live-model characterisation of malformed nested-object-array tool-call
//! arguments, against the real Gemma 4 E4B chat path.
//!
//! These record the measurement the fix rests on, so a later change to the chat
//! template, grammar, or model can be checked against the same evidence rather
//! than against a remembered claim:
//!
//! - A first attempt encodes `create_schema`'s array-of-objects `fields`
//!   cleanly.
//! - A retry reaches for whatever shape the prior assistant turn holds. With
//!   llama.cpp's own grammar that meant malformed in, malformed out: a key is
//!   any text up to the next `:`, so an over-quoted key or a whole
//!   Python-style argument list is a legal key. The narrowed key rule
//!   (`chat::constrain_gemma4_argument_keys`) makes those keys unreachable, so
//!   the retry is clean whatever the history holds.
//! - A string argument closed with a plain `"` instead of the model's `<|"|>`
//!   token never ends. Generation stops at the call's close marker
//!   (`chat::ends_gemma4_tool_call`), so the arguments arrive unterminated
//!   rather than with the model's answer inside them.
//!
//! `agent_loop::repair_over_quoted_keys` still repairs an over-quoted key from
//! an engine that applies no grammar; it is covered by fast unit tests there.
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

/// `search_nodes` with the parameters the agent declares for it, including the
/// array-of-objects `sorting` the kwargs-shaped call was reaching for.
fn search_nodes_tool() -> ToolSpec {
    ToolSpec {
        name: "search_nodes".to_string(),
        description: "Find, list, and filter nodes by title, type, or stored field value."
            .to_string(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Keyword to match against node titles. Pass an empty string to list every node of a type."},
                "node_type": {"type": "string", "description": "Filter by node type."},
                "sorting": {
                    "type": "array",
                    "description": "Optional sort configuration, applied in order.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "field": {"type": "string"},
                            "direction": {"type": "string", "enum": ["asc", "desc"]}
                        },
                        "required": ["field"]
                    }
                },
                "limit": {"type": "integer", "description": "Max results to return (default 50)"}
            },
            "required": []
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
/// Runs both arms in one pass, holding everything but the prior turn's shape
/// fixed. Under llama.cpp's own grammar the malformed arm retried malformed in
/// 8 of 8 trials and the clean arm in none: the model copies the shape it reads
/// back out of its own history. With the key rule narrowed a quote is no longer
/// legal in a key, so both arms retry cleanly. Asserting both here means
/// neither can rot unnoticed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the Gemma 4 E4B GGUF; run explicitly with --ignored --nocapture"]
async fn a_retry_cannot_copy_over_quoted_keys_from_the_prior_call() {
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
    for (arm, prior_args) in [
        ("malformed-prior", malformed_prior),
        ("clean-prior", clean_prior),
    ] {
        // Counted per trial, not per malformed field. Fusing the two would tie the
        // assertion to how many fields the model happens to emit, so a run with the
        // same behaviour but a different field count would fail while naming the
        // wrong conclusion — and could prompt removing a repair that is still needed.
        let mut malformed_trials = 0;
        let mut malformed_fields = 0;
        let mut retried = 0;
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
            retried += 1;
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
        assert!(
            retried > 0,
            "[{arm}] no trial retried the call, so nothing was measured"
        );
        assert_eq!(
            malformed_trials, 0,
            "[{arm}] a retry must not carry an over-quoted key; if the malformed arm \
             fails, the narrowed key rule is no longer reaching the sampler"
        );
    }
}

/// Every key in `value`, at every depth, that is not a plain name.
fn keys_that_are_not_names(value: &serde_json::Value, found: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(obj) => {
            for (key, child) in obj {
                let is_name = !key.is_empty()
                    && key
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                if !is_name {
                    found.push(key.clone());
                }
                keys_that_are_not_names(child, found);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                keys_that_are_not_names(item, found);
            }
        }
        _ => {}
    }
}

/// A call whose arguments were written as a Python-style argument list reached
/// the tool as one key holding the whole list:
/// `{"direction":false,"query=\"\",node_type=\"schema\",limit=50,sorting=[{\"field\"":null}`.
/// llama.cpp's Gemma 4 grammar accepts that, because a key is any text up to
/// the next `:`.
///
/// The model copies the shape of its own prior call, so replaying that call as
/// the prior turn is what makes it reach for the shape again.
///
/// The retry writes its keys as names — the narrowed key rule leaves it
/// nothing else — but closes a string with the `"` it read in the history, so
/// the string never ends. Measured before generation stopped at the close
/// marker: the "argument" ran on through the marker and several sentences of
/// answer to the 512-token cap, 26 seconds, in 4 of 4 trials. It now stops at
/// the marker with nothing after it, and the unterminated arguments are what
/// the agent loop reports back as a malformed call.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the Gemma 4 E4B GGUF; run explicitly with --ignored --nocapture"]
async fn a_retry_of_a_kwargs_shaped_call_stops_at_the_close_marker() {
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

    let reported_args = r#"{"direction":false,"query=\"\",node_type=\"schema\",limit=50,sorting=[{\"field\"":null}"#;
    serde_json::from_str::<serde_json::Value>(reported_args)
        .expect("the reported arguments are valid JSON, which is why nothing rejected them");

    const CLOSE_MARKER: &str = "<tool_call|>";
    let trials = 4;
    let mut calls = 0;
    for i in 0..trials {
        let messages = vec![
            // The two resident rules that make a turn retry a failed call
            // rather than answer in prose; without them there is no retry to
            // measure.
            ChatMessage::text(
                Role::System,
                "Call tools immediately when intent is clear. Tool call error: read the error \
                 message, fix your arguments, and retry ONCE.",
            ),
            ChatMessage::text(Role::User, "What schemas do we have here?"),
            ChatMessage {
                role: Role::Assistant,
                content: String::new(),
                tool_calls: vec![nodespace_nlp_engine::ToolCallRaw {
                    id: "call_1".to_string(),
                    function_name: "search_nodes".to_string(),
                    arguments_json: reported_args.to_string(),
                    provider_extra: None,
                }],
                tool_call_id: None,
                name: None,
                reasoning: None,
            },
            // What the tool answered when the call was dispatched to it.
            ChatMessage {
                role: Role::Tool,
                content: "{\"error\":\"invalid arguments for tool search_nodes: unknown field \
                          `direction`, expected one of `query`, `node_type`, `sorting`, `limit`\"}"
                    .to_string(),
                tool_calls: Vec::new(),
                tool_call_id: Some("call_1".to_string()),
                name: Some("search_nodes".to_string()),
                reasoning: None,
            },
        ];

        let raw = Arc::new(Mutex::new(String::new()));
        let args = Arc::new(Mutex::new(String::new()));
        let (r, a) = (Arc::clone(&raw), Arc::clone(&args));
        service
            .generate_streaming(
                messages,
                Some(vec![search_nodes_tool()]),
                0.0,
                512,
                move |chunk| match chunk {
                    ChatChunk::Token(t) => r.lock().unwrap().push_str(&t),
                    ChatChunk::ToolCallArgs { json, .. } => a.lock().unwrap().push_str(&json),
                    _ => {}
                },
            )
            .await
            .expect("generation must succeed");

        let joined = args.lock().unwrap().clone();
        println!("--- kwargs retry trial {i} ---");
        println!("args: {joined}");
        println!("text: {:?}", raw.lock().unwrap());

        if joined.is_empty() {
            continue;
        }
        calls += 1;

        if let Some((_, after_marker)) = joined.split_once(CLOSE_MARKER) {
            assert!(
                after_marker.is_empty(),
                "generation must stop at the call's close marker; the arguments ran on with \
                 {after_marker:?}"
            );
        }

        // Unterminated arguments do not parse, and there is then no key to
        // check: the agent loop reports those as malformed. Arguments that do
        // parse must not hold an argument list or prose as a key.
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&joined) {
            let mut not_names = Vec::new();
            keys_that_are_not_names(&parsed, &mut not_names);
            assert!(
                not_names.is_empty(),
                "a retry must not carry an argument list or prose as a key; got {not_names:?}"
            );
        }
    }

    assert!(
        calls > 0,
        "no trial retried the call, so nothing was measured"
    );
}
