//! ReAct (Reason + Act) loop and session management for the local agent.
//!
//! Orchestrates the conversation cycle: build prompts, call inference,
//! parse tool calls, execute tools, feed results back, and repeat until
//! the model produces a final response or hits iteration limits.
//!
//! Issue #1006

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::agent_types::{
    AgentSession, AgentToolExecutor, AgentTurnResult, ChatInferenceEngine, ChatMessage,
    InferenceError, InferenceRequest, InferenceUsage, LocalAgentStatus, Role, StreamingChunk,
    ToolCallRaw, ToolExecutionRecord,
};
use crate::local_agent::prompt_templates;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of tool-call iterations per turn.
const MAX_TOOL_ITERATIONS: usize = 5;

/// Total token budget for the context window.
const TOTAL_TOKEN_BUDGET: u32 = 8_000;

/// Tokens reserved for the system prompt and tool definitions.
const SYSTEM_PROMPT_BUDGET: u32 = 2_000;

/// Tokens available for conversation history.
const HISTORY_TOKEN_BUDGET: u32 = TOTAL_TOKEN_BUDGET - SYSTEM_PROMPT_BUDGET;

// ---------------------------------------------------------------------------
// LocalAgentLoop
// ---------------------------------------------------------------------------

/// Core ReAct loop implementation.
///
/// Stateless: operates on a provided session and delegates to the injected
/// inference engine and tool executor. The caller (`LocalAgentService`)
/// manages session state and persistence.
pub struct LocalAgentLoop<E: ChatInferenceEngine + ?Sized, T: AgentToolExecutor + ?Sized> {
    engine: Arc<E>,
    tool_executor: Arc<T>,
}

impl<E: ChatInferenceEngine + ?Sized, T: AgentToolExecutor + ?Sized> LocalAgentLoop<E, T> {
    pub fn new(engine: Arc<E>, tool_executor: Arc<T>) -> Self {
        Self {
            engine,
            tool_executor,
        }
    }

    /// Execute one full agent turn: inference + tool loop.
    ///
    /// Appends the user message to the session, builds the prompt, runs
    /// inference (potentially multiple rounds of tool calls), and returns
    /// the final response. The session is mutated in place with all
    /// intermediate messages.
    ///
    /// `on_status` is called for each status transition.
    /// `on_chunk` forwards streaming tokens to the caller.
    /// `cancel` can be used to abort mid-generation.
    pub async fn run_turn(
        &self,
        session: &mut AgentSession,
        user_message: &str,
        on_status: impl Fn(LocalAgentStatus) + Send + Sync + 'static,
        on_chunk: impl Fn(StreamingChunk) + Send + Sync + 'static,
        cancel: CancellationToken,
    ) -> Result<AgentTurnResult, InferenceError> {
        // Wrap on_chunk in Arc so it can be cloned into each iteration's callback
        let on_chunk = Arc::new(on_chunk);

        // Append user message
        session.messages.push(ChatMessage {
            role: Role::User,
            content: user_message.to_string(),
            tool_call_id: None,
            name: None,
        });

        // Get available tools
        let tools = self
            .tool_executor
            .available_tools()
            .await
            .unwrap_or_default();

        // Build system prompt with tool definitions
        let system_content = format!(
            "{}{}",
            prompt_templates::system_prompt(),
            prompt_templates::format_tool_definitions(&tools),
        );

        let mut all_tool_executions: Vec<ToolExecutionRecord> = Vec::new();
        let mut total_usage = InferenceUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
        };

        // ReAct loop: iterate up to MAX_TOOL_ITERATIONS
        for iteration in 0..MAX_TOOL_ITERATIONS {
            if cancel.is_cancelled() {
                return Err(InferenceError::Engine("cancelled".into()));
            }

            // Maybe summarize history if over budget
            self.maybe_summarize_history(session, &system_content)
                .await?;

            // Build message list: system + history
            let mut messages = vec![ChatMessage {
                role: Role::System,
                content: system_content.clone(),
                tool_call_id: None,
                name: None,
            }];
            messages.extend(session.messages.clone());

            // Status: Thinking
            on_status(LocalAgentStatus::Thinking);
            session.status = LocalAgentStatus::Thinking;

            // Collect chunks to parse tool calls from the response
            let collected_chunks: Arc<Mutex<Vec<StreamingChunk>>> =
                Arc::new(Mutex::new(Vec::new()));
            let collected_for_cb = Arc::clone(&collected_chunks);
            let on_chunk_clone = Arc::clone(&on_chunk);

            // Wrap on_chunk so we can also collect
            let chunk_callback: Box<dyn Fn(StreamingChunk) + Send> =
                Box::new(move |chunk: StreamingChunk| {
                    // Forward to caller
                    on_chunk_clone(chunk.clone());
                    // Collect for parsing
                    if let Ok(mut guard) = collected_for_cb.try_lock() {
                        guard.push(chunk);
                    };
                });

            let request = InferenceRequest {
                messages,
                tools: Some(tools.clone()),
                temperature: Some(0.1),
                max_tokens: Some(1024),
            };

            // Run inference
            let usage = self.engine.generate(request, chunk_callback).await?;

            total_usage.prompt_tokens += usage.prompt_tokens;
            total_usage.completion_tokens += usage.completion_tokens;

            // Parse collected chunks into text + tool calls
            let chunks = collected_chunks.lock().await;
            let (response_text, tool_calls) = Self::parse_chunks(&chunks);
            drop(chunks);

            if tool_calls.is_empty() {
                // No tool calls — final response
                on_status(LocalAgentStatus::Streaming);
                session.status = LocalAgentStatus::Streaming;

                // Append assistant response to history
                session.messages.push(ChatMessage {
                    role: Role::Assistant,
                    content: response_text.clone(),
                    tool_call_id: None,
                    name: None,
                });

                on_status(LocalAgentStatus::Idle);
                session.status = LocalAgentStatus::Idle;

                return Ok(AgentTurnResult {
                    response: response_text,
                    tool_calls_made: all_tool_executions,
                    usage: total_usage,
                });
            }

            // Append assistant message with tool call indication
            session.messages.push(ChatMessage {
                role: Role::Assistant,
                content: response_text.clone(),
                tool_call_id: None,
                name: None,
            });

            // Execute each tool call
            for tc in &tool_calls {
                if cancel.is_cancelled() {
                    return Err(InferenceError::Engine("cancelled".into()));
                }

                on_status(LocalAgentStatus::ToolExecution {
                    tool_name: tc.function_name.clone(),
                });
                session.status = LocalAgentStatus::ToolExecution {
                    tool_name: tc.function_name.clone(),
                };

                let start = Instant::now();
                let args: serde_json::Value =
                    serde_json::from_str(&tc.arguments_json).unwrap_or(serde_json::json!({}));

                let tool_result = self
                    .tool_executor
                    .execute(&tc.function_name, args.clone())
                    .await;

                let duration_ms = start.elapsed().as_millis() as u64;

                let (result_value, is_error) = match tool_result {
                    Ok(tr) => (tr.result, tr.is_error),
                    Err(e) => (serde_json::json!({"error": e.to_string()}), true),
                };

                let record = ToolExecutionRecord {
                    tool_call_id: tc.id.clone(),
                    name: tc.function_name.clone(),
                    args,
                    result: result_value.clone(),
                    is_error,
                    duration_ms,
                };

                session.tool_executions.push(record.clone());
                all_tool_executions.push(record);

                // Append tool result to history
                let tool_msg = prompt_templates::format_tool_result(
                    &tc.function_name,
                    &result_value,
                    is_error,
                );
                session.messages.push(ChatMessage {
                    role: Role::Tool,
                    content: tool_msg,
                    tool_call_id: Some(tc.id.clone()),
                    name: Some(tc.function_name.clone()),
                });
            }

            // If this was the last allowed iteration, return what we have
            if iteration == MAX_TOOL_ITERATIONS - 1 {
                on_status(LocalAgentStatus::Idle);
                session.status = LocalAgentStatus::Idle;

                return Ok(AgentTurnResult {
                    response: response_text,
                    tool_calls_made: all_tool_executions,
                    usage: total_usage,
                });
            }

            // Otherwise loop back for another inference round
        }

        // Should not reach here, but just in case
        on_status(LocalAgentStatus::Idle);
        session.status = LocalAgentStatus::Idle;

        Ok(AgentTurnResult {
            response: String::new(),
            tool_calls_made: all_tool_executions,
            usage: total_usage,
        })
    }

    /// Parse collected streaming chunks into response text and tool calls.
    fn parse_chunks(chunks: &[StreamingChunk]) -> (String, Vec<ToolCallRaw>) {
        let mut text = String::new();
        let mut tool_calls: Vec<ToolCallRaw> = Vec::new();
        // Accumulate tool call args by id
        // Use Vec to preserve tool call ordering (important for causal dependencies)
        let mut pending_calls: Vec<(String, String, String)> = Vec::new(); // (id, name, args_json)

        for chunk in chunks {
            match chunk {
                StreamingChunk::Token { text: t } => {
                    text.push_str(t);
                }
                StreamingChunk::ToolCallStart { id, name } => {
                    pending_calls.push((id.clone(), name.clone(), String::new()));
                }
                StreamingChunk::ToolCallArgs { id, args_json } => {
                    if let Some(call) = pending_calls.iter_mut().rev().find(|(cid, _, _)| cid == id) {
                        call.2.push_str(args_json);
                    }
                }
                StreamingChunk::Done { .. } | StreamingChunk::Error { .. } => {}
            }
        }

        // Convert accumulated tool calls into ToolCallRaw (order preserved)
        for (id, name, args_json) in pending_calls {
            tool_calls.push(ToolCallRaw {
                id,
                function_name: name,
                arguments_json: args_json,
            });
        }

        (text, tool_calls)
    }

    /// Summarize older history turns if the conversation exceeds the token budget.
    ///
    /// Estimates token count for the full history. If it exceeds
    /// `HISTORY_TOKEN_BUDGET`, summarizes older messages (keeping the most
    /// recent 2-3 turns) and replaces them with a single summary message.
    async fn maybe_summarize_history(
        &self,
        session: &mut AgentSession,
        system_content: &str,
    ) -> Result<(), InferenceError> {
        if session.messages.len() <= 4 {
            // Too few messages to need summarization
            return Ok(());
        }

        // Estimate token count of the full conversation
        let mut history_text = String::new();
        for msg in &session.messages {
            history_text.push_str(&msg.content);
            history_text.push(' ');
        }

        let history_tokens = self.engine.token_count(&history_text).await?;
        let system_tokens = self.engine.token_count(system_content).await?;

        if history_tokens + system_tokens <= TOTAL_TOKEN_BUDGET {
            return Ok(());
        }

        if history_tokens <= HISTORY_TOKEN_BUDGET {
            return Ok(());
        }

        // Need to summarize. Keep the last 3 messages verbatim, summarize the rest.
        let keep_count = 3.min(session.messages.len());
        let split_point = session.messages.len() - keep_count;

        let older_messages: Vec<ChatMessage> = session.messages.drain(..split_point).collect();

        // Build summarization text from older messages
        let mut summary_input = String::new();
        for msg in &older_messages {
            let role_str = match msg.role {
                Role::System => "System",
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::Tool => "Tool",
            };
            summary_input.push_str(&format!("{}: {}\n", role_str, msg.content));
        }

        let summary_prompt = prompt_templates::summarization_prompt(&summary_input);

        // Run a single-shot summarization inference (no tools)
        let summary_request = InferenceRequest {
            messages: vec![ChatMessage {
                role: Role::User,
                content: summary_prompt,
                tool_call_id: None,
                name: None,
            }],
            tools: None,
            temperature: Some(0.1),
            max_tokens: Some(512),
        };

        let summary_chunks: Arc<Mutex<Vec<StreamingChunk>>> = Arc::new(Mutex::new(Vec::new()));
        let summary_for_cb = Arc::clone(&summary_chunks);
        let cb: Box<dyn Fn(StreamingChunk) + Send> = Box::new(move |chunk: StreamingChunk| {
            if let Ok(mut guard) = summary_for_cb.try_lock() {
                guard.push(chunk);
            }
        });

        let _ = self.engine.generate(summary_request, cb).await?;

        let chunks = summary_chunks.lock().await;
        let (summary_text, _) = Self::parse_chunks(&chunks);
        drop(chunks);

        let summary_content = if summary_text.is_empty() {
            // Fallback: just note that history was truncated
            "Previous conversation context was summarized due to token limits.".to_string()
        } else {
            format!("[Conversation summary]: {}", summary_text)
        };

        // Prepend summary as a system-like message at the start of remaining history
        session.messages.insert(
            0,
            ChatMessage {
                role: Role::System,
                content: summary_content,
                tool_call_id: None,
                name: None,
            },
        );

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// LocalAgentService
// ---------------------------------------------------------------------------

/// Session management facade for the local agent.
///
/// Manages active sessions and provides a high-level API for creating,
/// resuming, and ending conversations. Delegates the actual ReAct loop
/// to [`LocalAgentLoop`].
pub struct LocalAgentService<E: ChatInferenceEngine + ?Sized, T: AgentToolExecutor + ?Sized> {
    sessions: RwLock<HashMap<String, AgentSession>>,
    agent_loop: LocalAgentLoop<E, T>,
    /// Per-session cancellation tokens.
    cancel_tokens: RwLock<HashMap<String, CancellationToken>>,
}

impl<E: ChatInferenceEngine + ?Sized + 'static, T: AgentToolExecutor + ?Sized + 'static> LocalAgentService<E, T> {
    pub fn new(engine: Arc<E>, tool_executor: Arc<T>) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            agent_loop: LocalAgentLoop::new(engine, tool_executor),
            cancel_tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new conversation session.
    ///
    /// Returns the session ID. If a model_id is provided, it is recorded
    /// in the session metadata.
    pub async fn create_session(&self, model_id: Option<String>) -> String {
        let session_id = uuid::Uuid::new_v4().to_string();
        let session = AgentSession {
            id: session_id.clone(),
            model_id,
            messages: Vec::new(),
            status: LocalAgentStatus::Idle,
            created_at: chrono::Utc::now(),
            tool_executions: Vec::new(),
        };

        let cancel = CancellationToken::new();
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), session);
        self.cancel_tokens
            .write()
            .await
            .insert(session_id.clone(), cancel);

        session_id
    }

    /// Send a user message and run the agent turn.
    ///
    /// Returns the agent's response after potentially multiple rounds
    /// of tool execution. Streams chunks and status updates via callbacks.
    pub async fn send_message(
        &self,
        session_id: &str,
        message: &str,
        on_status: impl Fn(LocalAgentStatus) + Send + Sync + 'static,
        on_chunk: impl Fn(StreamingChunk) + Send + Sync + 'static,
    ) -> Result<AgentTurnResult, InferenceError> {
        let cancel = {
            let tokens = self.cancel_tokens.read().await;
            tokens
                .get(session_id)
                .cloned()
                .ok_or_else(|| InferenceError::Engine(format!("session not found: {session_id}")))?
        };

        // Take session out for mutation, put it back after
        let mut session = {
            let mut sessions = self.sessions.write().await;
            sessions
                .remove(session_id)
                .ok_or_else(|| InferenceError::Engine(format!("session not found: {session_id}")))?
        };

        let result = self
            .agent_loop
            .run_turn(&mut session, message, on_status, on_chunk, cancel)
            .await;

        // Put session back
        self.sessions
            .write()
            .await
            .insert(session_id.to_string(), session);

        result
    }

    /// Cancel an in-progress generation for the given session.
    pub async fn cancel(&self, session_id: &str) {
        let mut tokens = self.cancel_tokens.write().await;
        if let Some(token) = tokens.get(session_id) {
            token.cancel();
        }
        // Replace with a fresh token for future use
        tokens.insert(session_id.to_string(), CancellationToken::new());
    }

    /// End and remove a session, freeing all resources.
    pub async fn end_session(&self, session_id: &str) {
        self.sessions.write().await.remove(session_id);
        if let Some(token) = self.cancel_tokens.write().await.remove(session_id) {
            token.cancel();
        }
    }

    /// List all active sessions (id + status).
    pub async fn get_sessions(&self) -> Vec<(String, LocalAgentStatus)> {
        self.sessions
            .read()
            .await
            .iter()
            .map(|(id, s)| (id.clone(), s.status.clone()))
            .collect()
    }

    /// Get a snapshot of a session's current state.
    pub async fn get_session(&self, session_id: &str) -> Option<AgentSession> {
        self.sessions.read().await.get(session_id).cloned()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_types::{ChatModelSpec, ToolDefinition, ToolError, ToolResult};
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // -- Mock inference engine -------------------------------------------

    /// Mock engine that returns pre-configured responses.
    struct MockEngine {
        /// Responses to return for sequential calls to `generate`.
        /// Each entry is a list of chunks to emit.
        responses: Mutex<Vec<Vec<StreamingChunk>>>,
        generate_count: AtomicUsize,
    }

    impl MockEngine {
        fn new(responses: Vec<Vec<StreamingChunk>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                generate_count: AtomicUsize::new(0),
            }
        }

        /// Create a mock that returns a single text response (no tools).
        fn single_text(text: &str) -> Self {
            Self::new(vec![vec![
                StreamingChunk::Token {
                    text: text.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]])
        }

        /// Create a mock that first returns a tool call, then a text response.
        fn tool_then_text(tool_name: &str, tool_args: &str, final_text: &str) -> Self {
            Self::new(vec![
                // First call: tool call
                vec![
                    StreamingChunk::ToolCallStart {
                        id: "tc_1".to_string(),
                        name: tool_name.to_string(),
                    },
                    StreamingChunk::ToolCallArgs {
                        id: "tc_1".to_string(),
                        args_json: tool_args.to_string(),
                    },
                    StreamingChunk::Done {
                        usage: InferenceUsage {
                            prompt_tokens: 20,
                            completion_tokens: 10,
                        },
                    },
                ],
                // Second call: final text
                vec![
                    StreamingChunk::Token {
                        text: final_text.to_string(),
                    },
                    StreamingChunk::Done {
                        usage: InferenceUsage {
                            prompt_tokens: 30,
                            completion_tokens: 15,
                        },
                    },
                ],
            ])
        }
    }

    #[async_trait]
    impl ChatInferenceEngine for MockEngine {
        async fn generate(
            &self,
            _request: InferenceRequest,
            on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            let idx = self.generate_count.fetch_add(1, Ordering::SeqCst);
            let responses = self.responses.lock().await;

            if idx >= responses.len() {
                // Return empty response if we run out of pre-configured ones
                on_chunk(StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 0,
                        completion_tokens: 0,
                    },
                });
                return Ok(InferenceUsage {
                    prompt_tokens: 0,
                    completion_tokens: 0,
                });
            }

            let chunks = &responses[idx];
            let mut usage = InferenceUsage {
                prompt_tokens: 0,
                completion_tokens: 0,
            };
            for chunk in chunks {
                if let StreamingChunk::Done { usage: u } = chunk {
                    usage = *u;
                }
                on_chunk(chunk.clone());
            }
            Ok(usage)
        }

        async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
            Ok(Some(ChatModelSpec {
                model_id: "test-model".into(),
                context_window: 8192,
                default_temperature: 0.1,
            }))
        }

        async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
            // Rough estimate: ~4 chars per token
            Ok((text.len() as f32 / 4.0).ceil() as u32)
        }
    }

    // -- Mock tool executor ----------------------------------------------

    struct MockToolExecutor {
        tools: Vec<ToolDefinition>,
        /// Canned results keyed by tool name.
        results: HashMap<String, serde_json::Value>,
    }

    impl MockToolExecutor {
        fn new() -> Self {
            let mut results = HashMap::new();
            results.insert(
                "search_nodes".to_string(),
                json!({"count": 2, "nodes": [
                    {"id": "abc123", "title": "Billing Architecture", "type": "text"},
                    {"id": "def456", "title": "Payment Processing", "type": "text"},
                ]}),
            );
            results.insert(
                "get_node".to_string(),
                json!({"id": "abc123", "title": "Billing Architecture", "body": "Content here"}),
            );

            Self {
                tools: vec![
                    ToolDefinition {
                        name: "search_nodes".into(),
                        description: "Search for nodes".into(),
                        parameters_schema: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
                    },
                    ToolDefinition {
                        name: "get_node".into(),
                        description: "Get a node by ID".into(),
                        parameters_schema: json!({"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}),
                    },
                ],
                results,
            }
        }
    }

    #[async_trait]
    impl AgentToolExecutor for MockToolExecutor {
        async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
            Ok(self.tools.clone())
        }

        async fn execute(
            &self,
            name: &str,
            _args: serde_json::Value,
        ) -> Result<ToolResult, ToolError> {
            let result = self
                .results
                .get(name)
                .cloned()
                .unwrap_or(json!({"error": "unknown tool"}));
            let is_error = !self.results.contains_key(name);
            Ok(ToolResult {
                tool_call_id: format!("call_{name}"),
                name: name.to_string(),
                result,
                is_error,
            })
        }
    }

    // -- Helper to create a fresh session --------------------------------

    fn new_session() -> AgentSession {
        AgentSession {
            id: "test-session".to_string(),
            model_id: Some("test-model".to_string()),
            messages: Vec::new(),
            status: LocalAgentStatus::Idle,
            created_at: chrono::Utc::now(),
            tool_executions: Vec::new(),
        }
    }

    // -- Tests -----------------------------------------------------------

    #[tokio::test]
    async fn single_turn_no_tools() {
        let engine = Arc::new(MockEngine::single_text("Hello! How can I help?"));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let statuses: Arc<std::sync::Mutex<Vec<LocalAgentStatus>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let statuses_cb = Arc::clone(&statuses);
        let chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let chunks_cb = Arc::clone(&chunks);

        let result = agent_loop
            .run_turn(
                &mut session,
                "Hi there",
                move |s| {
                    statuses_cb.lock().unwrap().push(s);
                },
                move |c| {
                    chunks_cb.lock().unwrap().push(c);
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.response, "Hello! How can I help?");
        assert!(result.tool_calls_made.is_empty());
        assert!(result.usage.prompt_tokens > 0);

        // Session should have 2 messages: user + assistant
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[1].role, Role::Assistant);
        assert_eq!(session.status, LocalAgentStatus::Idle);
    }

    #[tokio::test]
    async fn tool_call_then_final_response() {
        let engine = Arc::new(MockEngine::tool_then_text(
            "search_nodes",
            r#"{"query":"billing"}"#,
            "Found 2 nodes about billing.",
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Search for billing info",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert_eq!(result.response, "Found 2 nodes about billing.");
        assert_eq!(result.tool_calls_made.len(), 1);
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");

        // Session should have: user, assistant (tool call), tool result, assistant (final)
        assert_eq!(session.messages.len(), 4);
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[1].role, Role::Assistant);
        assert_eq!(session.messages[2].role, Role::Tool);
        assert_eq!(session.messages[3].role, Role::Assistant);
    }

    #[tokio::test]
    async fn multi_step_tool_chain() {
        // First: search_nodes, Second: get_node, Third: final text
        let engine = Arc::new(MockEngine::new(vec![
            // Round 1: search_nodes
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"query":"architecture"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
            // Round 2: get_node
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_2".to_string(),
                    name: "get_node".to_string(),
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_2".to_string(),
                    args_json: r#"{"id":"abc123"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 40,
                        completion_tokens: 10,
                    },
                },
            ],
            // Round 3: final response
            vec![
                StreamingChunk::Token {
                    text: "The Billing Architecture node describes...".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 60,
                        completion_tokens: 20,
                    },
                },
            ],
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Tell me about the architecture",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(result
            .response
            .contains("Billing Architecture node describes"));
        assert_eq!(result.tool_calls_made.len(), 2);
        assert_eq!(result.tool_calls_made[0].name, "search_nodes");
        assert_eq!(result.tool_calls_made[1].name, "get_node");

        // Total usage should sum all rounds
        assert_eq!(result.usage.prompt_tokens, 120); // 20+40+60
        assert_eq!(result.usage.completion_tokens, 40); // 10+10+20
    }

    #[tokio::test]
    async fn max_iteration_limit() {
        // All 5 rounds return tool calls, never a text response
        let tool_round = || {
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc".to_string(),
                    name: "search_nodes".to_string(),
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc".to_string(),
                    args_json: r#"{"query":"test"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]
        };

        let engine = Arc::new(MockEngine::new(vec![
            tool_round(),
            tool_round(),
            tool_round(),
            tool_round(),
            tool_round(),
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Keep searching",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // Should have executed exactly 5 tool calls (the limit)
        assert_eq!(result.tool_calls_made.len(), 5);
        // All should be search_nodes
        for tc in &result.tool_calls_made {
            assert_eq!(tc.name, "search_nodes");
        }
    }

    #[tokio::test]
    async fn cancellation_stops_generation() {
        let engine = Arc::new(MockEngine::single_text("Should not complete"));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let cancel = CancellationToken::new();
        cancel.cancel(); // Cancel immediately

        let result = agent_loop
            .run_turn(&mut session, "Hello", |_| {}, |_| {}, cancel)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            InferenceError::Engine(msg) => assert_eq!(msg, "cancelled"),
            other => panic!("Expected Engine error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn parse_chunks_text_only() {
        let chunks = vec![
            StreamingChunk::Token {
                text: "Hello ".to_string(),
            },
            StreamingChunk::Token {
                text: "world".to_string(),
            },
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 5,
                    completion_tokens: 2,
                },
            },
        ];
        let (text, tool_calls) =
            LocalAgentLoop::<MockEngine, MockToolExecutor>::parse_chunks(&chunks);
        assert_eq!(text, "Hello world");
        assert!(tool_calls.is_empty());
    }

    #[tokio::test]
    async fn parse_chunks_with_tool_calls() {
        let chunks = vec![
            StreamingChunk::Token {
                text: "Let me search".to_string(),
            },
            StreamingChunk::ToolCallStart {
                id: "tc_1".to_string(),
                name: "search_nodes".to_string(),
            },
            StreamingChunk::ToolCallArgs {
                id: "tc_1".to_string(),
                args_json: r#"{"query":""#.to_string(),
            },
            StreamingChunk::ToolCallArgs {
                id: "tc_1".to_string(),
                args_json: r#"test"}"#.to_string(),
            },
            StreamingChunk::Done {
                usage: InferenceUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                },
            },
        ];
        let (text, tool_calls) =
            LocalAgentLoop::<MockEngine, MockToolExecutor>::parse_chunks(&chunks);
        assert_eq!(text, "Let me search");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function_name, "search_nodes");
        assert_eq!(tool_calls[0].arguments_json, r#"{"query":"test"}"#);
    }

    // -- LocalAgentService tests -----------------------------------------

    #[tokio::test]
    async fn service_create_and_list_sessions() {
        let engine = Arc::new(MockEngine::single_text("Hello"));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let id1 = service.create_session(Some("model-a".into())).await;
        let id2 = service.create_session(None).await;

        let sessions = service.get_sessions().await;
        assert_eq!(sessions.len(), 2);

        let session1 = service.get_session(&id1).await.unwrap();
        assert_eq!(session1.model_id, Some("model-a".to_string()));

        let session2 = service.get_session(&id2).await.unwrap();
        assert_eq!(session2.model_id, None);
    }

    #[tokio::test]
    async fn service_end_session() {
        let engine = Arc::new(MockEngine::single_text("Hello"));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let id = service.create_session(None).await;
        assert!(service.get_session(&id).await.is_some());

        service.end_session(&id).await;
        assert!(service.get_session(&id).await.is_none());
        assert!(service.get_sessions().await.is_empty());
    }

    #[tokio::test]
    async fn service_send_message() {
        let engine = Arc::new(MockEngine::single_text("I can help with that!"));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let id = service.create_session(None).await;
        let result = service
            .send_message(&id, "Help me", |_| {}, |_| {})
            .await
            .unwrap();

        assert_eq!(result.response, "I can help with that!");

        // Session should still exist with messages
        let session = service.get_session(&id).await.unwrap();
        assert_eq!(session.messages.len(), 2);
    }

    #[tokio::test]
    async fn service_send_message_unknown_session() {
        let engine = Arc::new(MockEngine::single_text("Hello"));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let result = service
            .send_message("nonexistent", "Hello", |_| {}, |_| {})
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn service_cancel_session() {
        let engine = Arc::new(MockEngine::single_text("Hello"));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let id = service.create_session(None).await;

        // Cancel should not panic even if nothing is in progress
        service.cancel(&id).await;

        // Session should still be usable after cancel
        let session = service.get_session(&id).await;
        assert!(session.is_some());
    }

    #[tokio::test]
    async fn status_transitions_single_turn() {
        let engine = Arc::new(MockEngine::single_text("Response"));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let statuses: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let statuses_cb = Arc::clone(&statuses);

        agent_loop
            .run_turn(
                &mut session,
                "Hello",
                move |s| {
                    let label = match &s {
                        LocalAgentStatus::Idle => "Idle",
                        LocalAgentStatus::Thinking => "Thinking",
                        LocalAgentStatus::ToolExecution { .. } => "ToolExecution",
                        LocalAgentStatus::Streaming => "Streaming",
                        LocalAgentStatus::Error { .. } => "Error",
                    };
                    statuses_cb.lock().unwrap().push(label.to_string());
                },
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let statuses = statuses.lock().unwrap();
        // Should be: Thinking, Streaming, Idle
        assert!(statuses.contains(&"Thinking".to_string()));
        assert!(statuses.contains(&"Streaming".to_string()));
        assert!(statuses.contains(&"Idle".to_string()));
    }

    #[tokio::test]
    async fn status_transitions_with_tool() {
        let engine = Arc::new(MockEngine::tool_then_text(
            "search_nodes",
            r#"{"query":"test"}"#,
            "Done",
        ));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let statuses: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let statuses_cb = Arc::clone(&statuses);

        agent_loop
            .run_turn(
                &mut session,
                "Search",
                move |s| {
                    let label = match &s {
                        LocalAgentStatus::Idle => "Idle",
                        LocalAgentStatus::Thinking => "Thinking",
                        LocalAgentStatus::ToolExecution { .. } => "ToolExecution",
                        LocalAgentStatus::Streaming => "Streaming",
                        LocalAgentStatus::Error { .. } => "Error",
                    };
                    statuses_cb.lock().unwrap().push(label.to_string());
                },
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let statuses = statuses.lock().unwrap();
        // Should include: Thinking (round 1), ToolExecution, Thinking (round 2), Streaming, Idle
        assert!(statuses.contains(&"Thinking".to_string()));
        assert!(statuses.contains(&"ToolExecution".to_string()));
        assert!(statuses.contains(&"Idle".to_string()));
    }

    #[tokio::test]
    async fn history_summarization_trigger() {
        // Create an engine that always returns text (no tools) but we
        // pre-populate the session with enough history to trigger summarization.
        let engine = Arc::new(MockEngine::new(vec![
            // Summarization call
            vec![
                StreamingChunk::Token {
                    text: "Summary: user discussed billing and payments.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 50,
                        completion_tokens: 20,
                    },
                },
            ],
            // Actual response
            vec![
                StreamingChunk::Token {
                    text: "Here is your answer.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 30,
                        completion_tokens: 10,
                    },
                },
            ],
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();

        // Add enough history to exceed the token budget.
        // With ~4 chars/token estimate, we need > 6000*4 = 24000 chars of history.
        // 20 messages * 2000 chars = 40000 chars = ~10000 tokens > 6000 budget.
        for i in 0..20 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            session.messages.push(ChatMessage {
                role,
                content: format!("Message {} with extensive content: {}", i, "x".repeat(2000)),
                tool_call_id: None,
                name: None,
            });
        }

        let messages_before = session.messages.len();

        let result = agent_loop
            .run_turn(
                &mut session,
                "What was discussed?",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // After summarization, older messages should be replaced with a summary.
        // Without summarization, count would be: 20 (pre-existing) + 1 (user) + 1 (assistant) = 22.
        // With summarization, it should be: 1 (summary) + 3 (kept) + 1 (user) + 1 (assistant) = 6 or similar.
        assert!(
            session.messages.len() < messages_before,
            "Expected summarization to reduce message count. Before: {}, After: {}",
            messages_before,
            session.messages.len()
        );

        // The first message should be the summary
        assert!(
            session.messages[0]
                .content
                .contains("[Conversation summary]"),
            "First message should be the summary, got: {}",
            session.messages[0].content
        );

        assert_eq!(result.response, "Here is your answer.");
    }

    // -- Additional coverage tests ------------------------------------------

    /// Mock engine that always fails on generate.
    struct FailingEngine;

    #[async_trait]
    impl ChatInferenceEngine for FailingEngine {
        async fn generate(
            &self,
            _request: InferenceRequest,
            _on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            Err(InferenceError::Engine("model crashed".into()))
        }

        async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
            Ok(None)
        }

        async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
            Ok((text.len() as f32 / 4.0).ceil() as u32)
        }
    }

    /// When `run_turn` returns an error the session must still be in the
    /// sessions map (the "take-mutate-return" pattern reinserts on error).
    #[tokio::test]
    async fn session_persistence_after_inference_error() {
        let engine = Arc::new(FailingEngine);
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        let id = service.create_session(Some("test-model".into())).await;

        // send_message should fail because FailingEngine errors
        let result = service
            .send_message(&id, "Hello", |_| {}, |_| {})
            .await;

        assert!(result.is_err(), "Expected inference error");

        // Session must still exist in the map despite the error
        let session = service.get_session(&id).await;
        assert!(
            session.is_some(),
            "Session should persist after inference error"
        );

        // The user message should have been appended before the error
        let session = session.unwrap();
        assert!(
            !session.messages.is_empty(),
            "User message should be in session history"
        );
        assert_eq!(session.messages[0].role, Role::User);
        assert_eq!(session.messages[0].content, "Hello");
    }

    /// When every turn produces tool calls the loop must stop after exactly
    /// MAX_TOOL_ITERATIONS (5) and return without spinning forever.
    #[tokio::test]
    async fn max_iteration_limit_enforced_exactly() {
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_for_engine = Arc::clone(&call_count);

        // Build more than 5 rounds of tool-call responses so the engine
        // would happily keep going. The loop MUST stop at 5.
        let mut responses: Vec<Vec<StreamingChunk>> = Vec::new();
        for i in 0..8 {
            responses.push(vec![
                StreamingChunk::ToolCallStart {
                    id: format!("tc_{i}"),
                    name: "search_nodes".to_string(),
                },
                StreamingChunk::ToolCallArgs {
                    id: format!("tc_{i}"),
                    args_json: r#"{"query":"loop"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ]);
        }

        /// Mock engine that counts how many times generate is called.
        struct CountingEngine {
            inner: MockEngine,
            count: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl ChatInferenceEngine for CountingEngine {
            async fn generate(
                &self,
                request: InferenceRequest,
                on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
            ) -> Result<InferenceUsage, InferenceError> {
                self.count.fetch_add(1, Ordering::SeqCst);
                self.inner.generate(request, on_chunk).await
            }

            async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
                self.inner.model_info().await
            }

            async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
                self.inner.token_count(text).await
            }
        }

        let engine = Arc::new(CountingEngine {
            inner: MockEngine::new(responses),
            count: Arc::clone(&call_count),
        });
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Keep searching forever",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // Exactly 5 tool calls should have been made (one per iteration)
        assert_eq!(result.tool_calls_made.len(), 5);

        // Engine should have been called exactly 5 times (the loop constant)
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            MAX_TOOL_ITERATIONS,
            "generate should be called exactly MAX_TOOL_ITERATIONS times"
        );

        // Usage should be summed from all 5 rounds
        assert_eq!(result.usage.prompt_tokens, 50); // 10 * 5
        assert_eq!(result.usage.completion_tokens, 25); // 5 * 5
    }

    /// Cancellation during tool execution should stop the loop promptly.
    #[tokio::test]
    async fn cancellation_during_tool_execution() {
        // Engine returns a tool call in the first round
        let engine = Arc::new(MockEngine::new(vec![
            // Round 1: tool call
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_1".to_string(),
                    args_json: r#"{"query":"test"}"#.to_string(),
                },
                // Also request a second tool call in the same round
                StreamingChunk::ToolCallStart {
                    id: "tc_2".to_string(),
                    name: "get_node".to_string(),
                },
                StreamingChunk::ToolCallArgs {
                    id: "tc_2".to_string(),
                    args_json: r#"{"id":"abc123"}"#.to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
        ]));

        /// Executor that cancels the token after executing the first tool.
        struct CancellingExecutor {
            inner: MockToolExecutor,
            cancel: CancellationToken,
            call_count: AtomicUsize,
        }

        #[async_trait]
        impl AgentToolExecutor for CancellingExecutor {
            async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
                self.inner.available_tools().await
            }

            async fn execute(
                &self,
                name: &str,
                args: serde_json::Value,
            ) -> Result<ToolResult, ToolError> {
                let count = self.call_count.fetch_add(1, Ordering::SeqCst);
                let result = self.inner.execute(name, args).await;
                // Cancel after the first tool execution
                if count == 0 {
                    self.cancel.cancel();
                }
                result
            }
        }

        let cancel = CancellationToken::new();
        let executor = Arc::new(CancellingExecutor {
            inner: MockToolExecutor::new(),
            cancel: cancel.clone(),
            call_count: AtomicUsize::new(0),
        });
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(&mut session, "Search", |_| {}, |_| {}, cancel)
            .await;

        // Should have been cancelled
        assert!(result.is_err(), "Expected cancellation error");
        match result.unwrap_err() {
            InferenceError::Engine(msg) => assert_eq!(msg, "cancelled"),
            other => panic!("Expected Engine(cancelled), got {:?}", other),
        }
    }

    /// After summarization, the history token count should be below the budget.
    #[tokio::test]
    async fn history_summarization_reduces_token_count() {
        let engine = Arc::new(MockEngine::new(vec![
            // Summarization call — return a short summary
            vec![
                StreamingChunk::Token {
                    text: "User asked about billing.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 50,
                        completion_tokens: 10,
                    },
                },
            ],
            // Actual response
            vec![
                StreamingChunk::Token {
                    text: "Here you go.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 30,
                        completion_tokens: 5,
                    },
                },
            ],
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(Arc::clone(&engine), executor);

        let mut session = new_session();

        // Fill history with enough content to exceed HISTORY_TOKEN_BUDGET.
        // ~4 chars/token, budget is 6000 tokens => need > 24000 chars.
        for i in 0..30 {
            let role = if i % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            session.messages.push(ChatMessage {
                role,
                content: format!("Msg {}: {}", i, "a".repeat(2000)),
                tool_call_id: None,
                name: None,
            });
        }

        let _result = agent_loop
            .run_turn(
                &mut session,
                "Summarize everything",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await
            .unwrap();

        // Calculate token count of post-summarization history
        let mut total_text = String::new();
        for msg in &session.messages {
            total_text.push_str(&msg.content);
            total_text.push(' ');
        }
        let token_count = engine.token_count(&total_text).await.unwrap();

        assert!(
            token_count <= HISTORY_TOKEN_BUDGET,
            "After summarization, history tokens ({}) should be at or below budget ({})",
            token_count,
            HISTORY_TOKEN_BUDGET
        );
    }

    /// Tool calls with empty or invalid JSON args should be handled gracefully
    /// (defaulting to `{}` rather than panicking).
    #[tokio::test]
    async fn empty_tool_call_args_handled_gracefully() {
        // Engine returns a tool call with empty args, then a final text response
        let engine = Arc::new(MockEngine::new(vec![
            // Round 1: tool call with empty args string
            vec![
                StreamingChunk::ToolCallStart {
                    id: "tc_1".to_string(),
                    name: "search_nodes".to_string(),
                },
                // No ToolCallArgs chunks at all — args_json will be ""
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Round 2: final text
            vec![
                StreamingChunk::Token {
                    text: "Done with empty args.".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 20,
                        completion_tokens: 10,
                    },
                },
            ],
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let agent_loop = LocalAgentLoop::new(engine, executor);

        let mut session = new_session();
        let result = agent_loop
            .run_turn(
                &mut session,
                "Do something",
                |_| {},
                |_| {},
                CancellationToken::new(),
            )
            .await;

        // Should not panic — empty args_json falls back to json!({})
        assert!(result.is_ok(), "Empty tool call args should not cause panic");
        let result = result.unwrap();
        assert_eq!(result.response, "Done with empty args.");
        assert_eq!(result.tool_calls_made.len(), 1);
        // Args should have been defaulted to empty object
        assert_eq!(result.tool_calls_made[0].args, json!({}));
    }

    /// Two sessions can exist and operate independently without interference.
    #[tokio::test]
    async fn multiple_concurrent_sessions() {
        // Engine produces different responses based on call order:
        // calls 0,1 are for session A and session B respectively.
        let engine = Arc::new(MockEngine::new(vec![
            // Session A's response
            vec![
                StreamingChunk::Token {
                    text: "Response for session A".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 10,
                        completion_tokens: 5,
                    },
                },
            ],
            // Session B's response
            vec![
                StreamingChunk::Token {
                    text: "Response for session B".to_string(),
                },
                StreamingChunk::Done {
                    usage: InferenceUsage {
                        prompt_tokens: 15,
                        completion_tokens: 8,
                    },
                },
            ],
        ]));
        let executor = Arc::new(MockToolExecutor::new());
        let service = LocalAgentService::new(engine, executor);

        // Create two independent sessions
        let id_a = service.create_session(Some("model-a".into())).await;
        let id_b = service.create_session(Some("model-b".into())).await;

        assert_ne!(id_a, id_b, "Session IDs should be unique");

        // Send a message to session A
        let result_a = service
            .send_message(&id_a, "Hello from A", |_| {}, |_| {})
            .await
            .unwrap();

        // Send a message to session B
        let result_b = service
            .send_message(&id_b, "Hello from B", |_| {}, |_| {})
            .await
            .unwrap();

        // Verify responses are independent
        assert_eq!(result_a.response, "Response for session A");
        assert_eq!(result_b.response, "Response for session B");

        // Verify each session has its own history
        let session_a = service.get_session(&id_a).await.unwrap();
        let session_b = service.get_session(&id_b).await.unwrap();

        assert_eq!(session_a.messages.len(), 2); // user + assistant
        assert_eq!(session_b.messages.len(), 2); // user + assistant

        assert_eq!(session_a.messages[0].content, "Hello from A");
        assert_eq!(session_b.messages[0].content, "Hello from B");

        assert_eq!(session_a.model_id, Some("model-a".to_string()));
        assert_eq!(session_b.model_id, Some("model-b".to_string()));

        // Ending session A should not affect session B
        service.end_session(&id_a).await;
        assert!(service.get_session(&id_a).await.is_none());
        assert!(service.get_session(&id_b).await.is_some());

        // Session B should still be functional
        let sessions = service.get_sessions().await;
        assert_eq!(sessions.len(), 1);
    }
}
