use crate::agent_types::{
    ChatInferenceEngine, ChatModelSpec, InferenceError, InferenceRequest, InferenceUsage, Role,
    StreamingChunk,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

pub struct OllamaInferenceEngine {
    http_client: reqwest::Client,
    base_url: String,
    model_name: String,
}

impl OllamaInferenceEngine {
    pub fn new(model_name: String) -> Self {
        Self {
            http_client: reqwest::Client::new(),
            base_url: "http://127.0.0.1:11434".to_string(),
            model_name,
        }
    }

    pub fn with_base_url(model_name: String, base_url: String) -> Self {
        Self {
            http_client: reqwest::Client::new(),
            base_url,
            model_name,
        }
    }
}

// Private types for Ollama API serialization/deserialization
#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OllamaTool>>,
    stream: bool,
    /// Disable chain-of-thought thinking for thinking models (e.g. gemma4).
    /// Thinking tokens count against num_predict and cause long delays on
    /// tool-calling requests without adding value for structured outputs.
    think: bool,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct OllamaTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OllamaFunction,
}

#[derive(Serialize)]
struct OllamaFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

#[derive(Deserialize)]
struct OllamaChatChunk {
    message: Option<OllamaMessageChunk>,
    done: bool,
    #[serde(default)]
    prompt_eval_count: u32,
    #[serde(default)]
    eval_count: u32,
}

#[derive(Deserialize)]
struct OllamaMessageChunk {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OllamaToolCall>,
}

#[derive(Deserialize)]
struct OllamaToolCall {
    function: OllamaToolCallFunction,
}

#[derive(Deserialize)]
struct OllamaToolCallFunction {
    name: String,
    arguments: serde_json::Value,
}

#[derive(Deserialize)]
struct OllamaShowResponse {
    modelinfo: Option<serde_json::Value>,
}

#[async_trait]
impl ChatInferenceEngine for OllamaInferenceEngine {
    async fn generate(
        &self,
        request: InferenceRequest,
        on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
    ) -> Result<InferenceUsage, InferenceError> {
        // Map ChatMessage to OllamaMessage
        let messages: Vec<OllamaMessage> = request
            .messages
            .iter()
            .map(|msg| OllamaMessage {
                role: map_role(&msg.role),
                content: msg.content.clone(),
                tool_call_id: msg.tool_call_id.clone(),
            })
            .collect();

        // Map ToolDefinition to OllamaTool
        let tools = request.tools.map(|tool_defs| {
            tool_defs
                .iter()
                .map(|t| OllamaTool {
                    tool_type: "function".to_string(),
                    function: OllamaFunction {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters_schema.clone(),
                    },
                })
                .collect()
        });

        let ollama_request = OllamaChatRequest {
            model: &self.model_name,
            messages,
            tools,
            stream: true,
            think: false,
            options: OllamaOptions {
                temperature: request.temperature,
                num_predict: request.max_tokens,
            },
        };

        let url = format!("{}/api/chat", self.base_url);

        // Log the full request payload at DEBUG level so we can inspect exactly
        // what is sent to Ollama (model, messages, tools, options).
        if tracing::enabled!(tracing::Level::DEBUG) {
            if let Ok(json) = serde_json::to_string_pretty(&ollama_request) {
                tracing::debug!(model = %self.model_name, payload = %json, "Ollama request");
            }
        }
        // Always log message count + system prompt length at INFO to make it
        // easy to see the prompt size in production logs without full verbosity.
        let system_len = ollama_request
            .messages
            .first()
            .filter(|m| m.role == "system")
            .map(|m| m.content.len())
            .unwrap_or(0);
        tracing::info!(
            model = %self.model_name,
            message_count = ollama_request.messages.len(),
            system_prompt_bytes = system_len,
            tool_count = ollama_request.tools.as_ref().map(|t| t.len()).unwrap_or(0),
            "Sending request to Ollama"
        );

        let response = self
            .http_client
            .post(&url)
            .json(&ollama_request)
            .send()
            .await
            .map_err(|e| InferenceError::Engine(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(InferenceError::Engine(format!(
                "Ollama API error {}: {}",
                status, body
            )));
        }

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut final_usage = InferenceUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        };

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| InferenceError::Engine(e.to_string()))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim().to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                match serde_json::from_str::<OllamaChatChunk>(&line) {
                    Ok(chunk_data) => {
                        // Process message content
                        if let Some(msg) = &chunk_data.message {
                            if let Some(content) = &msg.content {
                                if !content.is_empty() {
                                    on_chunk(StreamingChunk::Token {
                                        text: content.clone(),
                                    });
                                }
                            }

                            // Process tool calls
                            for (i, tool_call) in msg.tool_calls.iter().enumerate() {
                                let call_id = format!("call_{}", i);
                                on_chunk(StreamingChunk::ToolCallStart {
                                    id: call_id.clone(),
                                    name: tool_call.function.name.clone(),
                                });

                                let args_json =
                                    serde_json::to_string(&tool_call.function.arguments)
                                        .unwrap_or_default();
                                on_chunk(StreamingChunk::ToolCallArgs {
                                    id: call_id,
                                    args_json,
                                });
                            }
                        }

                        if chunk_data.done {
                            final_usage = InferenceUsage {
                                prompt_tokens: chunk_data.prompt_eval_count,
                                completion_tokens: chunk_data.eval_count,
                            };
                            on_chunk(StreamingChunk::Done { usage: final_usage });
                        }
                    }
                    Err(e) => tracing::warn!("Failed to parse Ollama chunk: {e}"),
                }
            }
        }

        Ok(final_usage)
    }

    async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
        let url = format!("{}/api/show", self.base_url);
        let show_request = serde_json::json!({ "name": self.model_name });

        match self.http_client.post(&url).json(&show_request).send().await {
            Ok(response) => match response.json::<OllamaShowResponse>().await {
                Ok(show_response) => {
                    let mut context_window = 4096u32;

                    if let Some(modelinfo) = show_response.modelinfo {
                        if let Some(ctx_len) =
                            modelinfo.get("llm.context_length").and_then(|v| v.as_u64())
                        {
                            context_window = ctx_len as u32;
                        }
                    }

                    Ok(Some(ChatModelSpec {
                        model_id: self.model_name.clone(),
                        context_window,
                        default_temperature: 0.7,
                    }))
                }
                Err(e) => {
                    tracing::warn!(
                        "ollama: failed to parse /api/show response for '{}': {e}",
                        self.model_name
                    );
                    Ok(None)
                }
            },
            Err(_) => Ok(None), // daemon unreachable — model_info is optional
        }
    }

    async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
        Ok((text.len() / 4) as u32)
    }
}

fn map_role(role: &Role) -> String {
    match role {
        Role::System => "system".to_string(),
        Role::User => "user".to_string(),
        Role::Assistant => "assistant".to_string(),
        Role::Tool => "tool".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_count_estimate() {
        let engine = OllamaInferenceEngine::new("llama3.2:3b".to_string());
        let text = "hello world"; // len=11, 11/4=2 by integer division
        let count = futures::executor::block_on(engine.token_count(text))
            .expect("token_count should succeed");
        assert_eq!(
            count,
            2,
            "token_count should be text.len()/4 = {}",
            text.len() / 4
        );
    }

    #[test]
    fn test_map_chat_message_roles() {
        assert_eq!(map_role(&Role::System), "system");
        assert_eq!(map_role(&Role::User), "user");
        assert_eq!(map_role(&Role::Assistant), "assistant");
        assert_eq!(map_role(&Role::Tool), "tool");
    }

    #[test]
    fn test_parse_ollama_chat_chunk_token() {
        let json = r#"{
            "message": {
                "content": "hello",
                "tool_calls": []
            },
            "done": false,
            "prompt_eval_count": 0,
            "eval_count": 0
        }"#;

        let chunk: OllamaChatChunk =
            serde_json::from_str(json).expect("should deserialize OllamaChatChunk");
        assert!(!chunk.done);
        assert_eq!(chunk.eval_count, 0);
        assert_eq!(chunk.prompt_eval_count, 0);

        if let Some(msg) = chunk.message {
            assert_eq!(msg.content, Some("hello".to_string()));
            assert!(msg.tool_calls.is_empty());
        } else {
            panic!("message should not be None");
        }
    }

    #[test]
    fn test_parse_ollama_chat_chunk_done() {
        let json = r#"{
            "message": null,
            "done": true,
            "prompt_eval_count": 10,
            "eval_count": 20
        }"#;

        let chunk: OllamaChatChunk =
            serde_json::from_str(json).expect("should deserialize OllamaChatChunk");
        assert!(chunk.done);
        assert_eq!(chunk.prompt_eval_count, 10);
        assert_eq!(chunk.eval_count, 20);
        assert!(chunk.message.is_none());
    }

    #[test]
    fn test_parse_ollama_tool_call() {
        let json = r#"{
            "message": {
                "content": null,
                "tool_calls": [
                    {
                        "function": {
                            "name": "search",
                            "arguments": {"query": "test"}
                        }
                    }
                ]
            },
            "done": false,
            "prompt_eval_count": 0,
            "eval_count": 0
        }"#;

        let chunk: OllamaChatChunk =
            serde_json::from_str(json).expect("should deserialize OllamaChatChunk");

        if let Some(msg) = chunk.message {
            assert_eq!(msg.tool_calls.len(), 1);
            let tool_call = &msg.tool_calls[0];
            assert_eq!(tool_call.function.name, "search");
            assert_eq!(
                tool_call
                    .function
                    .arguments
                    .get("query")
                    .and_then(|v| v.as_str()),
                Some("test")
            );
        } else {
            panic!("message should not be None");
        }
    }
}
