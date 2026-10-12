//! Executes a [`GoldenCase`] against the real inference path and prints what
//! came back. It does not judge — the human reads the output and decides.
//!
//! In the path: `LlamaChatInferenceEngine` → `nlp-engine::ChatEngine`, which
//! applies the model's own chat template (Gemma 4 → `PEG_GEMMA4`), runs
//! llama.cpp's tool-call parsing, and — via [`repair_tool_call_arguments`]
//! applied here at the same parse boundary `agent_loop.rs` applies it —
//! production's argument repairs.
//!
//! Out: daemon, database, gRPC, `LocalAgentLoop`, `PromptAssembler`, routing,
//! chat-node lifecycle. None of them change what the model sees.
//!
//! The fidelity line sits at the *template*, and that is load-bearing: tuning
//! against a hand-rolled llama.cpp call or `llama-server` HTTP applies a
//! different template and parser. ADR-038's findings were gathered that way,
//! and ADR-046 recorded that they did not transfer through NodeSpace's own
//! inference path. That is the entire reason this is a Rust bin, not curl.

use std::path::Path;
use std::sync::{Arc, Mutex};

use nodespace_nlp_engine::chat::{ChatConfig, ChatMessage, Role, ToolCallRaw};

use crate::local_agent::agent_loop::repair_tool_call_arguments;
use nodespace_nlp_engine::inference::local::LlamaChatInferenceEngine;
use nodespace_nlp_engine::{
    ChatInferenceEngine, InferenceRequest, ModelFamily, StreamingChunk, ToolDefinition,
};

use super::case::{CaseError, GoldenCase, DEFAULT_TOOL_RESULT};

/// The ADR-056 locked native model, at the path the golden tests use.
pub fn default_model_path() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.nodespace/models/gemma-4-E4B-it-Q4_K_M.gguf")
}

/// Context window for the runner's engine. Matches the golden tests'.
pub const RUNNER_N_CTX: u32 = 32768;

/// Load the engine once per process. Loading is slow (a ~5GB GGUF plus Metal
/// kernel compilation), so every rep of every turn shares one.
pub fn load_engine(model_path: &Path) -> Result<LlamaChatInferenceEngine, CaseError> {
    let config = ChatConfig {
        n_ctx: RUNNER_N_CTX,
        default_temperature: 0.1,
        ..Default::default()
    };
    let path = model_path.to_string_lossy().to_string();
    LlamaChatInferenceEngine::load(
        &path,
        ModelFamily::Gemma4,
        config,
        crate::local_agent::model_manager::expected_sha256_for_path(&path),
    )
    .map_err(|e| CaseError::Io(format!("could not load model at {path}: {e}")))
}

/// A tool call as observed from a turn, after production's argument repairs.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedCall {
    /// The tool the model named.
    pub name: String,
    /// The repaired arguments JSON, exactly as it would reach the tool.
    pub arguments_json: String,
    /// The arguments as the model emitted them, before repair, when repair
    /// changed something — `None` when the emission was already clean.
    ///
    /// #2182: without this the two are indistinguishable in the output, and a
    /// case where the model still emits a malformation that production quietly
    /// repairs reads as a clean pass. That is the difference between "the
    /// prompt fixed it" and "the repair fixed it", which is exactly what an
    /// arm measuring a prompt change has to be able to tell apart.
    pub raw_arguments_json: Option<String>,
}

/// Everything a single turn produced.
#[derive(Debug, Clone, Default)]
pub struct TurnOutput {
    /// The assistant's text, if any.
    pub text: String,
    /// Tool calls in emission order.
    pub calls: Vec<ObservedCall>,
}

impl TurnOutput {
    /// One line describing what the model did.
    ///
    /// "no tool call parsed" is spelled out rather than shown as an empty
    /// list, because it is a distinct thing to see: it points at the template
    /// or the parser, where a wrong tool name points at the prompt.
    pub fn summary(&self) -> String {
        if self.calls.is_empty() {
            let text = self.text.trim();
            if text.is_empty() {
                return "no tool call parsed, and no text".to_string();
            }
            return format!("no tool call parsed; text: {text}");
        }
        self.calls
            .iter()
            .map(|c| match &c.raw_arguments_json {
                // Both halves, because they answer different questions: the
                // repaired args are what the tool ran, the emitted ones are
                // what the prompt actually produced. An arm tuning the prompt
                // is judged on the second.
                Some(raw) => format!(
                    "{}({})  [emitted, before repair: {}]",
                    c.name, c.arguments_json, raw
                ),
                None => format!("{}({})", c.name, c.arguments_json),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Run every rep of a case, printing each turn's result as it completes.
///
/// Printing as it goes rather than at the end matters at this cadence: a
/// multi-turn case is minutes of wall clock, and the first rep is usually
/// enough to tell whether an edit went the right way.
pub async fn run_case(engine: &LlamaChatInferenceEngine, case: &GoldenCase) {
    for rep in 1..=case.reps {
        run_rep(engine, case, rep).await;
    }
}

async fn run_rep(engine: &LlamaChatInferenceEngine, case: &GoldenCase, rep: u32) {
    // Messages accumulated from earlier turns of THIS rep: each turn's real
    // user message, its real assistant output, and the tool results paired to
    // the calls it actually made. A later turn therefore reads what the model
    // really said, not a hand-picked stand-in for it.
    let mut carried: Vec<ChatMessage> = Vec::new();

    for (i, turn) in case.turns.iter().enumerate() {
        let tools: Vec<ToolDefinition> = turn
            .tools
            .iter()
            .filter_map(|t| t.to_tool_definition().ok())
            .collect();

        let mut messages = vec![ChatMessage::text(Role::System, turn.system.clone())];
        messages.extend(turn.history.iter().map(|h| h.to_chat_message()));
        messages.extend(carried.iter().cloned());
        messages.push(ChatMessage::text(Role::User, turn.user.clone()));

        let output = generate(engine, messages, tools, case.temperature, case.max_tokens).await;

        println!(
            "[{}] rep {}/{} {}: {}",
            case.name,
            rep,
            case.reps,
            case.turn_label(i),
            output.summary()
        );
        // Printed next to the result, never compared against it. The reader
        // does the comparing — that is the whole loop.
        if !turn.expect.is_empty() {
            println!("    (tuned to produce: {})", turn.expect);
        }

        carried.push(ChatMessage::text(Role::User, turn.user.clone()));
        carried.push(assistant_turn(&output));
        for (index, call) in output.calls.iter().enumerate() {
            let result = turn
                .tool_results
                .get(&call.name)
                .cloned()
                .unwrap_or_else(|| DEFAULT_TOOL_RESULT.to_string());
            carried.push(ChatMessage::tool_result(
                result,
                call_id(index),
                call.name.clone(),
            ));
        }
    }
}

/// Rebuild the assistant turn the model just produced, tool calls included.
///
/// Carrying the calls (rather than only the text) is what keeps the replayed
/// history well-formed: the template renders them into the assistant turn so
/// the `tool` messages that follow have a call to pair with. Orphaned tool
/// results destabilize generation on some models, which would show up as a
/// prompt finding that is really an artifact of this file.
fn assistant_turn(output: &TurnOutput) -> ChatMessage {
    if output.calls.is_empty() {
        return ChatMessage::text(Role::Assistant, output.text.clone());
    }
    let calls = output
        .calls
        .iter()
        .enumerate()
        .map(|(i, c)| ToolCallRaw {
            id: call_id(i),
            function_name: c.name.clone(),
            arguments_json: c.arguments_json.clone(),
            provider_extra: None,
        })
        .collect();
    ChatMessage::assistant_with_tool_calls(output.text.clone(), calls)
}

/// Deterministic ids for the replayed calls, so a tool result pairs with the
/// call it answers.
fn call_id(index: usize) -> String {
    format!("call_{index}")
}

/// One generation, collected and parsed the way production parses it.
async fn generate(
    engine: &LlamaChatInferenceEngine,
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    temperature: f32,
    max_tokens: u32,
) -> TurnOutput {
    let request = InferenceRequest {
        messages,
        tools: if tools.is_empty() { None } else { Some(tools) },
        temperature: Some(temperature),
        max_tokens: Some(max_tokens),
    };

    let chunks: Arc<Mutex<Vec<StreamingChunk>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = chunks.clone();
    let generated = engine
        .generate(
            request,
            Box::new(move |c| {
                if let Ok(mut g) = sink.lock() {
                    g.push(c);
                }
            }),
        )
        .await;

    let collected = {
        let guard = chunks.lock().unwrap_or_else(|p| p.into_inner());
        guard.clone()
    };

    if let Err(e) = generated {
        // Surfaced as text rather than a panic so one bad rep does not discard
        // the reps that already printed.
        return TurnOutput {
            text: format!("<inference error: {e}>"),
            calls: Vec::new(),
        };
    }

    parse_output(&collected)
}

/// Accumulate streamed chunks into text plus repaired tool calls.
///
/// Mirrors `LocalAgentLoop::parse_chunks` followed by the
/// [`repair_tool_call_arguments`] pass at `agent_loop.rs`'s parse boundary.
/// The accumulation is duplicated rather than shared because `parse_chunks` is
/// a private associated function on the loop, and widening the loop's surface
/// to reach twenty lines of chunk folding would couple this utility to a type
/// it deliberately keeps out of the path. The repair itself is *not*
/// duplicated — it is the same public function production calls, which is the
/// part that would change the result if it drifted.
fn parse_output(chunks: &[StreamingChunk]) -> TurnOutput {
    let mut text = String::new();
    // A Vec, not a map, so emission order survives.
    let mut pending: Vec<(String, String, String)> = Vec::new();

    for chunk in chunks {
        match chunk {
            StreamingChunk::Token { text: t } => text.push_str(t),
            StreamingChunk::ToolCallStart { id, name, .. } => {
                pending.push((id.clone(), name.clone(), String::new()));
            }
            StreamingChunk::ToolCallArgs { id, args_json } => {
                if let Some(call) = pending.iter_mut().rev().find(|(cid, _, _)| cid == id) {
                    call.2.push_str(args_json);
                }
            }
            // Reasoning is dropped rather than folded into `text`: it is the
            // model's scratchpad, not its reply, and mixing them would make
            // the printed output misleading.
            StreamingChunk::Reasoning { .. }
            | StreamingChunk::Done { .. }
            | StreamingChunk::Error { .. } => {}
        }
    }

    let calls = pending
        .into_iter()
        .map(|(_, name, mut arguments_json)| {
            let emitted = arguments_json.clone();
            repair_tool_call_arguments(&mut arguments_json);
            let raw_arguments_json = (arguments_json != emitted).then_some(emitted);
            ObservedCall {
                name,
                arguments_json,
                raw_arguments_json,
            }
        })
        .collect();

    TurnOutput { text, calls }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_nlp_engine::InferenceUsage;

    fn tool_start(id: &str, name: &str) -> StreamingChunk {
        StreamingChunk::ToolCallStart {
            id: id.into(),
            name: name.into(),
            provider_extra: None,
        }
    }

    fn tool_args(id: &str, args: &str) -> StreamingChunk {
        StreamingChunk::ToolCallArgs {
            id: id.into(),
            args_json: args.into(),
        }
    }

    #[test]
    fn parse_output_reassembles_streamed_argument_fragments() {
        let chunks = vec![
            tool_start("a", "resolve_query"),
            tool_args("a", r#"{"request":"#),
            tool_args("a", r#""the 2400 one"}"#),
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 1,
                    completion_tokens: 1,
                },
            },
        ];
        let out = parse_output(&chunks);
        assert_eq!(out.calls.len(), 1);
        assert_eq!(out.calls[0].name, "resolve_query");
        assert_eq!(out.calls[0].arguments_json, r#"{"request":"the 2400 one"}"#);
    }

    #[test]
    fn parse_output_preserves_emission_order_across_interleaved_calls() {
        let chunks = vec![
            tool_start("a", "resolve_query"),
            tool_start("b", "update_node"),
            tool_args("b", r#"{"id":"x"}"#),
            tool_args("a", r#"{"request":"y"}"#),
        ];
        let out = parse_output(&chunks);
        let names: Vec<&str> = out.calls.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["resolve_query", "update_node"]);
        assert_eq!(out.calls[1].arguments_json, r#"{"id":"x"}"#);
    }

    #[test]
    fn parse_output_applies_productions_argument_repairs() {
        // Over-quoted keys: the model wrote `"\"name\""` where `name` was
        // meant. Production repairs this at the parse boundary, so the printed
        // call must be the repaired one — otherwise the output shows a defect
        // no real turn would have had, and the prompt gets blamed for it.
        let chunks = vec![
            tool_start("a", "create_schema"),
            tool_args(
                "a",
                r#"{"fields":[{"\"name\"":"status","\"type\"":"text"}]}"#,
            ),
        ];
        let out = parse_output(&chunks);
        let parsed: serde_json::Value =
            serde_json::from_str(&out.calls[0].arguments_json).expect("repaired args parse");
        assert_eq!(parsed["fields"][0]["name"], "status");
        assert_eq!(parsed["fields"][0]["type"], "text");
    }

    #[test]
    fn parse_output_leaves_a_well_formed_call_byte_identical() {
        let raw = r#"{"request":"the 2400 one","node_type":"equipment"}"#;
        let out = parse_output(&[tool_start("a", "resolve_query"), tool_args("a", raw)]);
        assert_eq!(out.calls[0].arguments_json, raw);
    }

    #[test]
    fn parse_output_keeps_reasoning_out_of_the_printed_text() {
        let chunks = vec![
            StreamingChunk::Reasoning {
                text: "I should call update_node".into(),
            },
            StreamingChunk::Token {
                text: "Which item did you mean?".into(),
            },
        ];
        assert_eq!(parse_output(&chunks).text, "Which item did you mean?");
    }

    #[test]
    fn summary_distinguishes_no_tool_call_from_a_call() {
        let none = TurnOutput {
            text: "Could you give me the node id?".into(),
            calls: Vec::new(),
        };
        assert!(none.summary().starts_with("no tool call parsed"));
        assert!(none.summary().contains("node id"));

        let called = TurnOutput {
            text: String::new(),
            calls: vec![ObservedCall {
                name: "resolve_query".into(),
                arguments_json: r#"{"request":"x"}"#.into(),
                raw_arguments_json: None,
            }],
        };
        assert_eq!(called.summary(), r#"resolve_query({"request":"x"})"#);
    }

    /// #2182: a repaired call must not read as a clean one. The summary is the
    /// only thing the human judging an arm sees, so when repair moved something
    /// the emission has to appear beside the result — otherwise "the prompt
    /// produced correct arguments" and "the prompt produced a malformation
    /// production silently fixed" are the same line of output.
    #[test]
    fn summary_shows_the_emission_when_repair_changed_it() {
        let chunks = vec![
            tool_start("c0", "search_nodes"),
            tool_args(
                "c0",
                r#"{"filters":[{"operator":"in","property":"stage","value":"cut,soak"}]}"#,
            ),
        ];
        let output = parse_output(&chunks);
        let summary = output.summary();
        assert!(
            summary.contains(r#""value":["cut","soak"]"#),
            "the repaired args are what the tool ran; got {summary}"
        );
        assert!(
            summary.contains(r#"[emitted, before repair: {"filters":[{"operator":"in","property":"stage","value":"cut,soak"}]}]"#),
            "the raw emission is what the prompt produced and must stay visible; got {summary}"
        );
    }

    /// The converse: a clean emission must not grow a second half. An arm's
    /// output is compared byte-for-byte between reps, and an unconditional
    /// annotation would be noise on every line that has nothing to report.
    #[test]
    fn summary_stays_single_valued_when_nothing_was_repaired() {
        let chunks = vec![
            tool_start("c0", "search_nodes"),
            tool_args("c0", r#"{"node_type":"release","query":""}"#),
        ];
        let summary = parse_output(&chunks).summary();
        assert_eq!(
            summary,
            r#"search_nodes({"node_type":"release","query":""})"#
        );
        assert!(!summary.contains("before repair"));
    }

    #[test]
    fn a_replayed_tool_result_pairs_with_its_originating_call() {
        let output = TurnOutput {
            text: String::new(),
            calls: vec![
                ObservedCall {
                    name: "resolve_query".into(),
                    arguments_json: "{}".into(),
                    raw_arguments_json: None,
                },
                ObservedCall {
                    name: "update_node".into(),
                    arguments_json: "{}".into(),
                    raw_arguments_json: None,
                },
            ],
        };
        let assistant = assistant_turn(&output);
        let second = ChatMessage::tool_result("{}", call_id(1), "update_node");
        assert_eq!(
            second.tool_call_id.as_deref(),
            Some(assistant.tool_calls[1].id.as_str()),
            "an unpaired tool result is an artifact that would read as a prompt finding"
        );
    }
}
