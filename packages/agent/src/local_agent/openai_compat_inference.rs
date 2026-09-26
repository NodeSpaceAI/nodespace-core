//! `ChatInferenceEngine` implementation for user-configured OpenAI-compatible
//! HTTP endpoints (e.g. a self-hosted vLLM/LM Studio server, or Ollama's
//! `/v1` endpoint).
//!
//! This is the single path for every remotely-served model. Ollama is reached
//! here through its OpenAI-compatible `/v1` API rather than its native wire
//! format — one protocol implementation serves every such provider.

use crate::agent_types::{
    ChatInferenceEngine, ChatMessage, ChatModelSpec, InferenceError, InferenceRequest,
    InferenceUsage, ModelFamily, StreamingChunk,
};
use crate::local_agent::ndjson::NdjsonLineBuffer;
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Prefix used to identify OpenAI-compatible provider configs. The suffix is
/// the UUID of the config stored in daemon.toml (`[[openai_compat.configs]]`).
pub const OPENAI_COMPAT_PREFIX: &str = "openai-compat:";

/// Check if a model ID represents an OpenAI-compatible provider config.
pub fn is_openai_compat(model_id: &str) -> bool {
    model_id.starts_with(OPENAI_COMPAT_PREFIX)
}

/// Strip the `"openai-compat:"` prefix, returning everything after it.
///
/// Returns the original model ID unchanged if it does not have the prefix.
/// Prefer [`parse_openai_compat_id`] when you need the config UUID on its own —
/// the remainder may also carry a discovered model segment.
pub fn strip_openai_compat_prefix(model_id: &str) -> &str {
    model_id
        .strip_prefix(OPENAI_COMPAT_PREFIX)
        .unwrap_or(model_id)
}

/// Split an `openai-compat:` model ID into its config UUID and optional model.
///
/// Two forms are accepted:
/// - `openai-compat:<uuid>` — the config's own `model` field is used.
/// - `openai-compat:<uuid>:<model>` — a specific model discovered at that
///   endpoint, which is how one config exposes the several models a server
///   serves.
///
/// The split is on the **first** colon after the prefix: a UUID never contains
/// one, whereas a model identifier routinely does (`llama3.1:8b`), so anything
/// past that first separator belongs to the model.
pub fn parse_openai_compat_id(model_id: &str) -> (&str, Option<&str>) {
    let rest = strip_openai_compat_prefix(model_id);
    match rest.split_once(':') {
        Some((config_id, model)) if !model.is_empty() => (config_id, Some(model)),
        _ => (rest, None),
    }
}

pub struct OpenAiCompatInferenceEngine {
    http_client: reqwest::Client,
    /// Base URL of the endpoint, e.g. "https://api.openai.com/v1". Requests
    /// are sent to "<base_url>/chat/completions".
    base_url: String,
    api_key: String,
    /// Model identifier sent as the request body's "model" field — must be
    /// the provider's actual model id (e.g. "gpt-4o"), not a cosmetic UI
    /// label. Real OpenAI-API and multi-model servers reject or misroute an
    /// unrecognized value; single-model local servers (Ollama, LM Studio)
    /// generally ignore it.
    model_name: String,
}

/// Time allowed to establish a TCP connection to the endpoint.
///
/// Deliberately short: a wrong or dead `base_url` should surface as an error in
/// seconds rather than hanging the chat turn.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Time allowed for a single request, measured to the *end* of the response body.
///
/// A generation legitimately runs for minutes on a slow local model, so this is
/// far longer than a typical HTTP timeout. Its job is to bound the pathological
/// case the bare default could not: an endpoint that accepts the connection and
/// then goes silent forever. `reqwest` surfaces an error only on transport
/// failure, and a connected-but-stalled server is not one.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// Retries allowed after a 429 or 5xx before the turn is failed.
///
/// Retries FOLLOW the initial send, so this many retries means up to
/// `RETRY_MAX_ATTEMPTS + 1` requests. Combined with `RETRY_MAX_DELAY` the
/// worst case is 80s of waiting — long enough to ride out the burst limits
/// hosted free tiers enforce, short enough that a genuine outage surfaces as
/// an error instead of a hung run.
const RETRY_MAX_ATTEMPTS: u32 = 4;

/// First backoff wait; doubles per attempt (2s, 4s, 8s, 16s).
const RETRY_BASE_DELAY: Duration = Duration::from_secs(2);

/// Ceiling on any single wait, including a server-supplied `Retry-After`.
///
/// Bounds the WORST case, which is driven by `Retry-After` rather than the
/// backoff: the header replaces the computed delay, so `RETRY_MAX_ATTEMPTS`
/// waits of this length is the true maximum (80s at these values). The
/// exponential path alone would only reach 30s.
///
/// Deliberately below what a server might ask for. Honouring a full 60s
/// `Retry-After` four times over would stall a single turn for four minutes,
/// and the caller here is a measurement run — better to fail the turn and
/// record it than to hide two minutes of waiting inside one scenario.
const RETRY_MAX_DELAY: Duration = Duration::from_secs(20);

impl OpenAiCompatInferenceEngine {
    pub fn new(base_url: String, api_key: String, model_name: String) -> Self {
        Self {
            http_client: Self::build_http_client(),
            base_url,
            api_key,
            model_name,
        }
    }

    /// Build the HTTP client with connect and request timeouts applied.
    ///
    /// Falls back to a default client if the builder fails, which it does only
    /// on TLS backend initialization errors — losing the timeouts is strictly
    /// better than refusing to construct the engine at all.
    fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_else(|e| {
                // Near-unreachable (TLS backend init only), but log it: the
                // fallback client has no timeouts at all, which is precisely
                // the hang this constant set exists to prevent. Silent here
                // would make that invisible.
                tracing::warn!(error = %e, "failed to build HTTP client with timeouts; falling back to an untimed client");
                reqwest::Client::new()
            })
    }

    /// The wire-protocol model identifier this engine sends as the request
    /// body's "model" field. Exposed so callers (and tests) can distinguish
    /// it from a config's cosmetic `name` — see
    /// `LocalAgentService::load_model_and_collect_events`, which must pass
    /// `config.model`, not `config.name`, into [`Self::new`].
    pub fn model_name(&self) -> &str {
        &self.model_name
    }
}

// ---------------------------------------------------------------------------
// Private types for OpenAI chat-completions API serialization/deserialization
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OpenAiChatRequest<'a> {
    model: &'a str,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiTool>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiRequestToolCall>>,
}

/// An assistant message's outgoing tool call, replayed from history so a
/// subsequent `tool`-role result has a matching call in the same request —
/// required by strict OpenAI-compatible servers (see module docs on
/// `OpenAiMessage`).
#[derive(Serialize)]
struct OpenAiRequestToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OpenAiRequestToolCallFunction,
    /// Provider-opaque fields captured from the original response (e.g.
    /// Gemini 3's `extra_content.google.thought_signature`), echoed back
    /// verbatim as siblings of `id`/`type`/`function`. `None` when the
    /// provider attached nothing.
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    provider_extra: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct OpenAiRequestToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Serialize)]
struct OpenAiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAiFunction,
}

#[derive(Serialize)]
struct OpenAiFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// Shape shared by both the non-streaming response body and each streaming
/// SSE `data:` chunk — OpenAI's `chat.completion` and `chat.completion.chunk`
/// objects differ only in whether `message` or `delta` is populated.
#[derive(Deserialize)]
struct OpenAiChatResponse {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    #[serde(default)]
    message: Option<OpenAiResponseMessage>,
    #[serde(default)]
    delta: Option<OpenAiResponseMessage>,
}

#[derive(Deserialize, Default)]
struct OpenAiResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiToolCall>,
}

#[derive(Deserialize)]
struct OpenAiToolCall {
    #[serde(default)]
    id: Option<String>,
    /// Named explicitly (never read) so `flatten` below excludes it from
    /// `provider_extra` — the wire's `"type": "function"` is a KNOWN OpenAI
    /// field, not provider-opaque state. Omitting this field would let
    /// `flatten` sweep it into `provider_extra`, which then re-emits it
    /// alongside `OpenAiRequestToolCall`'s own hardcoded `call_type` field on
    /// replay, producing a JSON object with two `"type"` keys.
    #[serde(rename = "type", default)]
    _call_type: Option<String>,
    function: OpenAiToolCallFunction,
    /// Provider-opaque sibling fields beyond the OpenAI schema (`id`/`type`/
    /// `function`) — e.g. Gemini 3's `extra_content.google.thought_signature`,
    /// which its OpenAI-compat endpoint requires echoed back verbatim at the
    /// same path on the next turn or it 400s the whole conversation.
    /// Captured generically via `flatten`. Every field the wire format is
    /// known to send MUST be named above, or `flatten` silently treats it as
    /// "provider-opaque" too — see `_call_type`.
    #[serde(flatten)]
    provider_extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct OpenAiToolCallFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: String,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

/// Convert a captured `provider_extra` map into the `Option<Value>` carried on
/// `StreamingChunk`/`ToolCallRaw`, collapsing an empty map to `None` so a
/// provider that attached nothing round-trips as a clean no-op rather than an
/// empty JSON object.
fn provider_extra_value(
    extra: &serde_json::Map<String, serde_json::Value>,
) -> Option<serde_json::Value> {
    (!extra.is_empty()).then(|| serde_json::Value::Object(extra.clone()))
}

/// Map an internal history message to its OpenAI wire-format equivalent,
/// replaying tool calls and the tool-result `name` so a strict
/// OpenAI-compatible server accepts the replayed history — see the module
/// docs on [`OpenAiMessage`] for why this matters.
fn to_openai_message(msg: &ChatMessage) -> OpenAiMessage {
    OpenAiMessage {
        role: msg.role.as_str().to_string(),
        content: msg.content.clone(),
        tool_call_id: msg.tool_call_id.clone(),
        name: msg.name.clone(),
        tool_calls: (!msg.tool_calls.is_empty()).then(|| {
            msg.tool_calls
                .iter()
                .map(|tc| OpenAiRequestToolCall {
                    id: tc.id.clone(),
                    call_type: "function".to_string(),
                    function: OpenAiRequestToolCallFunction {
                        name: tc.function_name.clone(),
                        arguments: tc.arguments_json.clone(),
                    },
                    provider_extra: tc.provider_extra.clone(),
                })
                .collect()
        }),
    }
}

#[async_trait]
impl ChatInferenceEngine for OpenAiCompatInferenceEngine {
    async fn generate(
        &self,
        request: InferenceRequest,
        on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
    ) -> Result<InferenceUsage, InferenceError> {
        let messages: Vec<OpenAiMessage> = request.messages.iter().map(to_openai_message).collect();

        let tools = request.tools.map(|tool_defs| {
            tool_defs
                .iter()
                .map(|t| OpenAiTool {
                    tool_type: "function".to_string(),
                    function: OpenAiFunction {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters_schema.clone(),
                    },
                })
                .collect()
        });

        // Tool-calling responses are not reliably delivered incrementally by
        // every OpenAI-compatible server, so use the same non-streaming
        // fallback strategy when tools are present.
        let use_stream = tools.is_none();
        let openai_request = OpenAiChatRequest {
            model: &self.model_name,
            messages,
            tools,
            stream: use_stream,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
        };

        if tracing::enabled!(tracing::Level::DEBUG) {
            if let Ok(json) = serde_json::to_string_pretty(&openai_request) {
                tracing::debug!(base_url = %self.base_url, payload = %json, "OpenAI-compat request");
            }
        }
        // Dev-only full-fidelity dump (NODESPACE_PROMPT_DUMP) — see
        // openai_compat_prompt_dump's module doc. Complements the debug-log
        // line above: that line requires RUST_LOG=debug and is not persisted
        // beyond the log; this is a durable, structured record correlated
        // with its response via `dump_seq`.
        let dump_seq = crate::local_agent::openai_compat_prompt_dump::enabled().then(|| {
            let request_json =
                serde_json::to_value(&openai_request).unwrap_or(serde_json::Value::Null);
            crate::local_agent::openai_compat_prompt_dump::dump_request(
                &self.base_url,
                &request_json,
            )
        });
        tracing::info!(
            base_url = %self.base_url,
            message_count = openai_request.messages.len(),
            tool_count = openai_request.tools.as_ref().map(|t| t.len()).unwrap_or(0),
            "Sending request to OpenAI-compatible endpoint"
        );

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut req_builder = self.http_client.post(&url).json(&openai_request);
        if !self.api_key.is_empty() {
            req_builder = req_builder.bearer_auth(&self.api_key);
        }

        // Rate limits are transient, so a 429 is retried rather than failing
        // the turn. Without this a throttled endpoint yields a turn that called
        // no tools, which every negative assertion scores GREEN and every
        // positive one scores red -- an eval run that silently measures the
        // provider's capacity instead of the model.
        //
        // `Retry-After` is honoured when present; otherwise the wait doubles
        // from RETRY_BASE_DELAY. Both are capped by RETRY_MAX_DELAY so a
        // pathological header cannot stall a run indefinitely.
        let mut attempt: u32 = 0;
        let response = loop {
            let builder = req_builder
                .try_clone()
                .ok_or_else(|| InferenceError::Engine("request is not retryable".to_string()))?;
            let response = builder
                .send()
                .await
                .map_err(|e| InferenceError::Engine(e.to_string()))?;

            let status = response.status();
            if status.is_success() {
                break response;
            }

            let retryable =
                status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
            if !retryable || attempt >= RETRY_MAX_ATTEMPTS {
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "unknown error".to_string());
                return Err(InferenceError::Engine(format!(
                    "OpenAI-compat API error {}: {}",
                    status, body
                )));
            }

            let after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs);
            let backoff = RETRY_BASE_DELAY * 2u32.pow(attempt);
            let wait = after.unwrap_or(backoff).min(RETRY_MAX_DELAY);

            tracing::warn!(
                status = %status,
                attempt = attempt + 1,
                wait_secs = wait.as_secs_f32(),
                "OpenAI-compat endpoint throttled; backing off"
            );
            tokio::time::sleep(wait).await;
            attempt += 1;
        };

        let mut final_usage = InferenceUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        };

        if use_stream {
            // `use_stream` is false whenever `tools` is present (see above),
            // so a tool-calling response -- the only case where
            // `provider_extra` ever carries content -- never actually
            // reaches this branch today. The `provider_extra` wiring on the
            // streamed `ToolCallStart` chunks below is speculative/defensive
            // completeness for a future provider that streams tool calls; it
            // is not exercised by any test or by production traffic.
            let mut stream = response.bytes_stream();
            let mut buffer = NdjsonLineBuffer::new();
            // Accumulates streamed tool-call fragments by index, since the
            // SSE delta format sends id/name once and arguments incrementally.
            let mut tool_call_ids: Vec<Option<String>> = Vec::new();
            // Raw SSE payload for NODESPACE_PROMPT_DUMP -- see the request-side
            // dump above for why this exists alongside the debug-log line.
            let mut raw_response_accum = String::new();

            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| InferenceError::Engine(e.to_string()))?;
                let lines = buffer
                    .push(&chunk)
                    .map_err(|e| InferenceError::Engine(e.to_string()))?;

                for line in lines {
                    let Some(data) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let data = data.trim();
                    if data == "[DONE]" {
                        if let Some(seq) = dump_seq {
                            crate::local_agent::openai_compat_prompt_dump::dump_response(
                                seq,
                                &raw_response_accum,
                            );
                        }
                        on_chunk(StreamingChunk::Done { usage: final_usage });
                        return Ok(final_usage);
                    }
                    if dump_seq.is_some() {
                        if !raw_response_accum.is_empty() {
                            raw_response_accum.push('\n');
                        }
                        raw_response_accum.push_str(data);
                    }

                    match serde_json::from_str::<OpenAiChatResponse>(data) {
                        Ok(resp) => {
                            if let Some(usage) = &resp.usage {
                                final_usage = InferenceUsage {
                                    prompt_tokens: usage.prompt_tokens,
                                    completion_tokens: usage.completion_tokens,
                                };
                            }
                            for choice in &resp.choices {
                                if let Some(delta) = &choice.delta {
                                    if let Some(content) = &delta.content {
                                        if !content.is_empty() {
                                            on_chunk(StreamingChunk::Token {
                                                text: content.clone(),
                                            });
                                        }
                                    }
                                    for (i, tool_call) in delta.tool_calls.iter().enumerate() {
                                        if tool_call_ids.len() <= i {
                                            tool_call_ids.resize(i + 1, None);
                                        }
                                        if let Some(id) = &tool_call.id {
                                            tool_call_ids[i] = Some(id.clone());
                                            on_chunk(StreamingChunk::ToolCallStart {
                                                id: id.clone(),
                                                name: tool_call
                                                    .function
                                                    .name
                                                    .clone()
                                                    .unwrap_or_default(),
                                                provider_extra: provider_extra_value(
                                                    &tool_call.provider_extra,
                                                ),
                                            });
                                        }
                                        if !tool_call.function.arguments.is_empty() {
                                            let id = tool_call_ids
                                                .get(i)
                                                .cloned()
                                                .flatten()
                                                .unwrap_or_else(|| format!("call_{}", i));
                                            on_chunk(StreamingChunk::ToolCallArgs {
                                                id,
                                                args_json: tool_call.function.arguments.clone(),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => tracing::warn!("Failed to parse OpenAI-compat SSE chunk: {e}"),
                    }
                }
            }
            // Stream ended without an explicit [DONE] sentinel.
            if let Some(seq) = dump_seq {
                crate::local_agent::openai_compat_prompt_dump::dump_response(
                    seq,
                    &raw_response_accum,
                );
            }
            on_chunk(StreamingChunk::Done { usage: final_usage });
        } else {
            let body = response
                .text()
                .await
                .map_err(|e| InferenceError::Engine(e.to_string()))?;

            if let Some(seq) = dump_seq {
                crate::local_agent::openai_compat_prompt_dump::dump_response(seq, &body);
            }

            match serde_json::from_str::<OpenAiChatResponse>(&body) {
                Ok(resp) => {
                    if let Some(usage) = &resp.usage {
                        final_usage = InferenceUsage {
                            prompt_tokens: usage.prompt_tokens,
                            completion_tokens: usage.completion_tokens,
                        };
                    }
                    if let Some(choice) = resp.choices.first() {
                        if let Some(msg) = &choice.message {
                            if let Some(content) = &msg.content {
                                if !content.is_empty() {
                                    on_chunk(StreamingChunk::Token {
                                        text: content.clone(),
                                    });
                                }
                            }
                            for (i, tool_call) in msg.tool_calls.iter().enumerate() {
                                let call_id = tool_call
                                    .id
                                    .clone()
                                    .unwrap_or_else(|| format!("call_{}", i));
                                on_chunk(StreamingChunk::ToolCallStart {
                                    id: call_id.clone(),
                                    name: tool_call.function.name.clone().unwrap_or_default(),
                                    provider_extra: provider_extra_value(&tool_call.provider_extra),
                                });
                                on_chunk(StreamingChunk::ToolCallArgs {
                                    id: call_id,
                                    args_json: tool_call.function.arguments.clone(),
                                });
                            }
                        }
                    }
                    on_chunk(StreamingChunk::Done { usage: final_usage });
                }
                Err(e) => {
                    tracing::warn!("Failed to parse OpenAI-compat non-streaming response: {e}");
                    on_chunk(StreamingChunk::Done {
                        usage: InferenceUsage::default(),
                    });
                }
            }
        }

        Ok(final_usage)
    }

    async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
        // OpenAI-compatible servers have no standard equivalent of Ollama's
        // /api/show for context-window introspection. Callers fall back to a
        // conservative default via the None case.
        Ok(Some(ChatModelSpec {
            model_id: self.model_name.clone(),
            family: ModelFamily::OpenAiCompat,
            context_window: 32_768,
            default_temperature: 0.7,
            type_k: None,
            type_v: None,
        }))
    }

    async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
        Ok((text.len() / 4) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_types::{Role, ToolCallRaw};

    #[test]
    fn test_is_openai_compat_prefix() {
        assert!(is_openai_compat("openai-compat:abc-123"));
        assert!(!is_openai_compat("ollama:llama3.2:3b"));
        assert!(!is_openai_compat("gemma-4-e4b-q4km"));
        assert!(is_openai_compat("openai-compat:"));
    }

    #[test]
    fn test_strip_openai_compat_prefix() {
        assert_eq!(
            strip_openai_compat_prefix("openai-compat:abc-123"),
            "abc-123"
        );
        assert_eq!(strip_openai_compat_prefix("gemma"), "gemma");
    }

    #[test]
    fn test_parse_non_streaming_response_with_tool_calls() {
        let json = r#"{
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_abc",
                            "function": {
                                "name": "search",
                                "arguments": "{\"query\":\"test\"}"
                            }
                        }
                    ]
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        }"#;

        let resp: OpenAiChatResponse = serde_json::from_str(json).expect("should deserialize");
        let usage = resp.usage.expect("usage should be present");
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);

        let msg = resp.choices[0].message.as_ref().expect("message present");
        assert_eq!(msg.tool_calls.len(), 1);
        assert_eq!(msg.tool_calls[0].id.as_deref(), Some("call_abc"));
        assert_eq!(msg.tool_calls[0].function.name.as_deref(), Some("search"));
        assert_eq!(msg.tool_calls[0].function.arguments, "{\"query\":\"test\"}");
    }

    #[test]
    fn test_parse_streaming_delta_token() {
        let json = r#"{
            "choices": [{
                "delta": {"content": "hello"}
            }]
        }"#;

        let resp: OpenAiChatResponse = serde_json::from_str(json).expect("should deserialize");
        let delta = resp.choices[0].delta.as_ref().expect("delta present");
        assert_eq!(delta.content.as_deref(), Some("hello"));
        assert!(delta.tool_calls.is_empty());
    }

    /// The retry policy stays bounded.
    ///
    /// A hosted endpoint under load can 429 indefinitely. Retrying is right,
    /// retrying forever is not: an eval that hangs is worse than one that
    /// reports an error, because it consumes the operator's time instead of
    /// their attention. These bounds keep the worst case at 80s — every attempt
    /// honouring a `Retry-After` at the ceiling, which is the path that governs
    /// it rather than the exponential backoff.
    #[test]
    fn retry_policy_is_bounded() {
        // The WORST case is every attempt honouring a `Retry-After` at the
        // ceiling — the header replaces the backoff, so this branch governs
        // the bound. Asserting only over the exponential path (30s here) would
        // pass while the real maximum drifted, which is how the previous
        // version of this test missed a 240s bound it claimed was 120s.
        let worst_with_retry_after: u64 = RETRY_MAX_ATTEMPTS as u64 * RETRY_MAX_DELAY.as_secs();
        assert!(
            worst_with_retry_after <= 120,
            "worst-case retry wait {worst_with_retry_after}s exceeds the 2 minute bound"
        );

        let backoff_only: u64 = (0..RETRY_MAX_ATTEMPTS)
            .map(|a| {
                (RETRY_BASE_DELAY * 2u32.pow(a))
                    .min(RETRY_MAX_DELAY)
                    .as_secs()
            })
            .sum();
        assert!(
            backoff_only <= worst_with_retry_after,
            "the exponential path ({backoff_only}s) should not exceed the \
             Retry-After ceiling ({worst_with_retry_after}s)"
        );
        // Const blocks, not runtime asserts: both compare compile-time
        // constants, so the invariant is provable at build time and a
        // violation should break the build rather than wait for someone to
        // run the test.
        const {
            assert!(
                RETRY_MAX_ATTEMPTS >= 1,
                "a zero-attempt policy is just the old terminal-failure behaviour"
            );
        }
        const {
            assert!(
                RETRY_BASE_DELAY.as_secs() <= RETRY_MAX_DELAY.as_secs(),
                "base delay above the ceiling makes the ceiling meaningless"
            );
        }
    }

    #[test]
    fn test_token_count_estimate() {
        let engine = OpenAiCompatInferenceEngine::new(
            "http://127.0.0.1:11434/v1".to_string(),
            String::new(),
            "placeholder".to_string(),
        );
        let text = "hello world"; // len=11, 11/4=2 by integer division
        let count = futures::executor::block_on(engine.token_count(text))
            .expect("token_count should succeed");
        assert_eq!(count, 2);
    }

    #[test]
    fn model_name_reflects_the_constructor_arg_not_a_display_label() {
        // Regression guard: callers (LocalAgentService::load_model_and_collect_events)
        // must pass the config's wire-protocol `model` field into `new`, never the
        // cosmetic `name` field a user typed into the Settings UI.
        let engine = OpenAiCompatInferenceEngine::new(
            "https://api.openai.com/v1".to_string(),
            "sk-test".to_string(),
            "gpt-4o".to_string(),
        );
        assert_eq!(engine.model_name(), "gpt-4o");
    }

    #[test]
    fn replayed_assistant_tool_call_carries_tool_calls_on_the_wire() {
        // Regression test: a replayed assistant turn that
        // issued a tool call must serialize with a `tool_calls` array, or a
        // strict OpenAI-compatible server rejects the following `tool`
        // message with "must be a response to a preceding message with
        // tool_calls".
        let assistant_turn = ChatMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRaw {
                id: "call_abc".to_string(),
                function_name: "search_nodes".to_string(),
                arguments_json: r#"{"query":"Q3 budget"}"#.to_string(),
                provider_extra: None,
            }],
        );

        let wire = to_openai_message(&assistant_turn);
        let json = serde_json::to_value(&wire).expect("serializes");

        assert_eq!(json["role"], "assistant");
        let tool_calls = json["tool_calls"]
            .as_array()
            .expect("tool_calls must be present on a replayed assistant tool-call turn");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call_abc");
        assert_eq!(tool_calls[0]["type"], "function");
        assert_eq!(tool_calls[0]["function"]["name"], "search_nodes");
        assert_eq!(
            tool_calls[0]["function"]["arguments"],
            r#"{"query":"Q3 budget"}"#
        );
    }

    #[test]
    fn replayed_tool_result_carries_its_tool_name_on_the_wire() {
        // Regression test: the OpenAI wire format expects a
        // `name` field on `tool`-role messages; dropping it left the model
        // unable to see which tool a past result came from.
        let tool_result = ChatMessage::tool_result(r#"{"nodes":[]}"#, "call_abc", "search_nodes");

        let wire = to_openai_message(&tool_result);
        let json = serde_json::to_value(&wire).expect("serializes");

        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_abc");
        assert_eq!(
            json["name"], "search_nodes",
            "tool-role messages must carry the tool name on replay"
        );
    }

    #[test]
    fn a_plain_text_turn_omits_tool_calls_and_name() {
        // A non-tool turn must not grow a spurious `tool_calls` or `name`
        // field — only turns that actually carry that data should emit it.
        let turn = ChatMessage::text(Role::User, "hello");

        let wire = to_openai_message(&turn);
        let json = serde_json::to_value(&wire).expect("serializes");

        assert!(json.get("tool_calls").is_none());
        assert!(json.get("name").is_none());
        assert!(json.get("tool_call_id").is_none());
    }

    #[test]
    fn response_tool_call_captures_thought_signature_as_provider_extra() {
        // Regression test: Gemini 3's OpenAI-compat endpoint
        // nests thought_signature under a vendor extension --
        // tool_calls[i].extra_content.google.thought_signature -- not as a
        // bare sibling field. Its OpenAI-compat endpoint requires this
        // echoed back verbatim on replay; dropping it is what made every
        // second tool-using turn 400. `flatten` captures `extra_content`
        // whole without needing to know its internal shape.
        let json = r#"{
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [
                        {
                            "id": "call_abc",
                            "type": "function",
                            "function": {
                                "name": "search",
                                "arguments": "{\"query\":\"test\"}"
                            },
                            "extra_content": {
                                "google": {
                                    "thought_signature": "opaque-token-xyz"
                                }
                            }
                        }
                    ]
                }
            }]
        }"#;

        let resp: OpenAiChatResponse = serde_json::from_str(json).expect("should deserialize");
        let msg = resp.choices[0].message.as_ref().expect("message present");
        let extra = provider_extra_value(&msg.tool_calls[0].provider_extra)
            .expect("extra_content must be captured as provider_extra");
        assert_eq!(
            extra["extra_content"]["google"]["thought_signature"],
            "opaque-token-xyz"
        );
        // The known OpenAI fields must not leak into provider_extra --
        // only genuinely unknown sibling fields belong there.
        assert!(extra.get("id").is_none());
        assert!(extra.get("function").is_none());
    }

    #[test]
    fn response_tool_call_without_extra_fields_yields_none() {
        // The common case (local servers, non-Gemini providers): no unknown
        // sibling fields on the tool call, so provider_extra must collapse to
        // None rather than an empty object -- an empty object would still
        // round-trip as a spurious field on replay.
        let json = r#"{
            "choices": [{
                "message": {
                    "tool_calls": [
                        {"id": "call_abc", "function": {"name": "search", "arguments": "{}"}}
                    ]
                }
            }]
        }"#;

        let resp: OpenAiChatResponse = serde_json::from_str(json).expect("should deserialize");
        let msg = resp.choices[0].message.as_ref().expect("message present");
        assert!(provider_extra_value(&msg.tool_calls[0].provider_extra).is_none());
    }

    #[test]
    fn replayed_tool_call_echoes_thought_signature_back_verbatim() {
        // Regression test: a replayed assistant tool-call
        // turn must carry `extra_content.google.thought_signature` back at
        // the exact same path, or Gemini 3's OpenAI-compat endpoint 400s the
        // turn with "Function call is missing a thought_signature". Google's
        // own guidance: return it "exactly as it was received".
        let assistant_turn = ChatMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRaw {
                id: "call_abc".to_string(),
                function_name: "search_nodes".to_string(),
                arguments_json: r#"{"query":"Q3 budget"}"#.to_string(),
                provider_extra: Some(serde_json::json!({
                    "extra_content": {
                        "google": {
                            "thought_signature": "opaque-token-xyz"
                        }
                    }
                })),
            }],
        );

        let wire = to_openai_message(&assistant_turn);
        let json = serde_json::to_value(&wire).expect("serializes");

        let tool_call = &json["tool_calls"][0];
        assert_eq!(tool_call["id"], "call_abc");
        assert_eq!(tool_call["type"], "function");
        assert_eq!(tool_call["function"]["name"], "search_nodes");
        assert_eq!(
            tool_call["extra_content"]["google"]["thought_signature"], "opaque-token-xyz",
            "thought_signature must be echoed back at the same nested path on replay"
        );
    }

    #[test]
    fn replayed_tool_call_without_provider_extra_stays_clean() {
        // A tool call with no provider_extra must not grow a spurious empty
        // field on replay -- only calls that actually carry provider state
        // should emit anything beyond the standard OpenAI shape.
        let assistant_turn = ChatMessage::assistant_with_tool_calls(
            "",
            vec![ToolCallRaw {
                id: "call_abc".to_string(),
                function_name: "search_nodes".to_string(),
                arguments_json: r#"{"query":"Q3 budget"}"#.to_string(),
                provider_extra: None,
            }],
        );

        let wire = to_openai_message(&assistant_turn);
        let json = serde_json::to_value(&wire).expect("serializes");
        let tool_call = json["tool_calls"][0]
            .as_object()
            .expect("tool call is an object");

        assert_eq!(
            tool_call.len(),
            3,
            "expected exactly id/type/function with no extra fields, got {tool_call:?}"
        );
    }

    #[test]
    fn response_type_field_does_not_leak_into_provider_extra() {
        // Regression test: the wire's "type":"function" is a KNOWN OpenAI
        // field, not provider-opaque state. If OpenAiToolCall didn't name it
        // explicitly, `flatten` would sweep it into provider_extra, and
        // replay would then emit it a second time alongside
        // OpenAiRequestToolCall's own hardcoded `type` field -- a JSON
        // object with two "type" keys.
        let json = r#"{
            "choices": [{
                "message": {
                    "tool_calls": [
                        {
                            "id": "call_abc",
                            "type": "function",
                            "function": {"name": "search", "arguments": "{}"},
                            "extra_content": {"google": {"thought_signature": "tok"}}
                        }
                    ]
                }
            }]
        }"#;

        let resp: OpenAiChatResponse = serde_json::from_str(json).expect("should deserialize");
        let msg = resp.choices[0].message.as_ref().expect("message present");
        let extra = provider_extra_value(&msg.tool_calls[0].provider_extra)
            .expect("extra_content must still be captured");
        assert!(
            extra.get("type").is_none(),
            "the known `type` field must not leak into provider_extra, got {extra:?}"
        );
    }

    #[test]
    fn a_parsed_response_replays_without_duplicating_the_type_key() {
        // Regression test: chains a real parsed OpenAiToolCall into the
        // replay path (rather than hand-authoring provider_extra on each
        // side independently) to catch exactly the class of seam bug the
        // issue's postmortem warned a one-shot, isolated test would miss --
        // here, a duplicate "type" key that only appears when parse and
        // replay are exercised together.
        let json = r#"{
            "choices": [{
                "message": {
                    "tool_calls": [
                        {
                            "id": "call_abc",
                            "type": "function",
                            "function": {"name": "search", "arguments": "{\"q\":1}"},
                            "extra_content": {"google": {"thought_signature": "tok"}}
                        }
                    ]
                }
            }]
        }"#;
        let resp: OpenAiChatResponse = serde_json::from_str(json).expect("should deserialize");
        let parsed = &resp.choices[0]
            .message
            .as_ref()
            .expect("message present")
            .tool_calls[0];

        let tool_call = ToolCallRaw {
            id: parsed.id.clone().expect("id present"),
            function_name: parsed.function.name.clone().unwrap_or_default(),
            arguments_json: parsed.function.arguments.clone(),
            provider_extra: provider_extra_value(&parsed.provider_extra),
        };
        let assistant_turn = ChatMessage::assistant_with_tool_calls("", vec![tool_call]);

        let wire = to_openai_message(&assistant_turn);
        let json = serde_json::to_value(&wire).expect("serializes");
        let replayed = json["tool_calls"][0]
            .as_object()
            .expect("tool call is an object");

        let type_count = replayed.keys().filter(|k| *k == "type").count();
        assert_eq!(
            type_count, 1,
            "replayed tool call must have exactly one `type` key, got {replayed:?}"
        );
        assert_eq!(replayed["type"], "function");
        assert_eq!(
            replayed["extra_content"]["google"]["thought_signature"], "tok",
            "thought_signature must survive the full parse-then-replay round trip"
        );
    }
}
