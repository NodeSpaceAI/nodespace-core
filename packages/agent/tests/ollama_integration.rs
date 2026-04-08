//! Integration tests for the Ollama backend.
//!
//! These tests connect to a real Ollama daemon at `http://127.0.0.1:11434`.
//! Each test gracefully skips if Ollama is not running — no failures, just a
//! printed message. Run with `cargo test -p nodespace-agent --test ollama_integration`.

use async_trait::async_trait;
use nodespace_agent::agent_types::{
    AgentToolExecutor, ChatInferenceEngine, ChatMessage, InferenceRequest, ModelManager, Role,
    StreamingChunk, ToolDefinition, ToolError, ToolResult,
};
use nodespace_agent::local_agent::agent_loop::LocalAgentService;
use nodespace_agent::local_agent::composite_model_manager::CompositeModelManager;
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_agent::local_agent::model_manager::GgufModelManager;
use nodespace_agent::local_agent::ollama_inference::OllamaInferenceEngine;
use nodespace_agent::local_agent::ollama_model_manager::OllamaModelManager;
use nodespace_nlp_engine::chat::ChatConfig;
use std::collections::HashMap;
use std::sync::Arc;

/// Returns true if Ollama is reachable at localhost:11434.
async fn ollama_running() -> bool {
    reqwest::Client::new()
        .get("http://127.0.0.1:11434/api/tags")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .is_ok()
}

/// Returns the name of the first available Ollama model, if any.
async fn first_ollama_model() -> Option<String> {
    let manager = OllamaModelManager::new();
    let models = manager.list().await.ok()?;
    models.into_iter().next().map(|m| m.id)
}

// ---------------------------------------------------------------------------
// OllamaModelManager integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ollama_is_available() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_is_available: Ollama not running at localhost:11434");
        return;
    }
    let manager = OllamaModelManager::new();
    assert!(
        manager.is_available().await,
        "is_available() should return true"
    );
}

#[tokio::test]
async fn test_ollama_list_models() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_list_models: Ollama not running at localhost:11434");
        return;
    }
    let manager = OllamaModelManager::new();
    let models = manager.list().await.expect("list() should succeed");
    // Just check it returns without error — may be empty if no models pulled yet
    eprintln!("Ollama models found: {}", models.len());
    for m in &models {
        eprintln!("  - {} ({} bytes)", m.id, m.size_bytes);
    }
}

#[tokio::test]
async fn test_composite_list_includes_ollama_prefix() {
    if !ollama_running().await {
        eprintln!(
            "SKIP test_composite_list_includes_ollama_prefix: Ollama not running at localhost:11434"
        );
        return;
    }
    let gguf = Arc::new(GgufModelManager::new().expect("GgufModelManager::new()"));
    let ollama = Arc::new(OllamaModelManager::new());
    let composite = CompositeModelManager::new(gguf, ollama);

    let models = composite.list().await.expect("list() should succeed");

    // GGUF models should have no prefix
    let gguf_models: Vec<_> = models
        .iter()
        .filter(|m| !CompositeModelManager::is_ollama(&m.id))
        .collect();
    // Ollama models should have "ollama:" prefix
    let ollama_models: Vec<_> = models
        .iter()
        .filter(|m| CompositeModelManager::is_ollama(&m.id))
        .collect();

    eprintln!(
        "Composite list: {} GGUF + {} Ollama models",
        gguf_models.len(),
        ollama_models.len()
    );

    // All Ollama-prefixed models should start with "ollama:"
    for m in &ollama_models {
        assert!(
            m.id.starts_with("ollama:"),
            "Ollama model ID should start with 'ollama:': {}",
            m.id
        );
    }
}

#[tokio::test]
async fn test_ollama_recommended_model() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_recommended_model: Ollama not running at localhost:11434");
        return;
    }
    let manager = OllamaModelManager::new();
    let rec = manager
        .recommended_model()
        .await
        .expect("recommended_model()");
    assert!(
        !rec.is_empty(),
        "recommended_model() should return non-empty string"
    );
    eprintln!("Recommended Ollama model: {rec}");
}

// ---------------------------------------------------------------------------
// OllamaInferenceEngine integration tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ollama_inference_generate() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_inference_generate: Ollama not running at localhost:11434");
        return;
    }
    let Some(model_name) = first_ollama_model().await else {
        eprintln!("SKIP test_ollama_inference_generate: No Ollama models available");
        return;
    };
    eprintln!("Using model: {model_name}");

    let engine = OllamaInferenceEngine::new(model_name.clone());
    let request = InferenceRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            content: "Reply with exactly the word 'pong'. No other text.".to_string(),
            tool_call_id: None,
            name: None,
        }],
        tools: None,
        temperature: Some(0.0),
        max_tokens: Some(10),
    };

    let chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let chunks_clone = chunks.clone();

    let usage = engine
        .generate(
            request,
            Box::new(move |chunk| {
                chunks_clone.lock().unwrap().push(chunk);
            }),
        )
        .await
        .expect("generate() should succeed");

    let collected = chunks.lock().unwrap();
    eprintln!("Chunks received: {}", collected.len());
    eprintln!(
        "Usage: {} prompt + {} completion tokens",
        usage.prompt_tokens, usage.completion_tokens
    );

    // Should have received at least one token chunk
    let has_token = collected
        .iter()
        .any(|c| matches!(c, StreamingChunk::Token { .. }));
    assert!(has_token, "Should receive at least one Token chunk");
}

#[tokio::test]
async fn test_ollama_inference_with_tools() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_inference_with_tools: Ollama not running at localhost:11434");
        return;
    }
    let Some(model_name) = first_ollama_model().await else {
        eprintln!("SKIP test_ollama_inference_with_tools: No Ollama models available");
        return;
    };
    eprintln!("Using model for tool test: {model_name}");

    let engine = OllamaInferenceEngine::new(model_name);
    let tool = ToolDefinition {
        name: "get_weather".to_string(),
        description: "Get the current weather for a city".to_string(),
        parameters_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "City name" }
            },
            "required": ["city"]
        }),
    };

    let request = InferenceRequest {
        messages: vec![ChatMessage {
            role: Role::User,
            content: "What's the weather in London?".to_string(),
            tool_call_id: None,
            name: None,
        }],
        tools: Some(vec![tool]),
        temperature: Some(0.0),
        max_tokens: Some(100),
    };

    let chunks: Arc<std::sync::Mutex<Vec<StreamingChunk>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let chunks_clone = chunks.clone();

    let _usage = engine
        .generate(
            request,
            Box::new(move |chunk| {
                chunks_clone.lock().unwrap().push(chunk);
            }),
        )
        .await
        .expect("generate() with tools should succeed");

    let collected = chunks.lock().unwrap();
    eprintln!("Tool test chunks received: {}", collected.len());

    // Model may or may not call the tool — both are valid responses.
    // Not all models support tool calling; some return no chunks in that case.
    // Verify only that no Error chunks were emitted.
    let has_error = collected
        .iter()
        .any(|c| matches!(c, StreamingChunk::Error { .. }));
    assert!(!has_error, "Should not receive Error chunks");
    eprintln!(
        "Tool call chunks: {} (model may not support tool calling)",
        collected.len()
    );
}

#[tokio::test]
async fn test_ollama_model_info() {
    if !ollama_running().await {
        eprintln!("SKIP test_ollama_model_info: Ollama not running at localhost:11434");
        return;
    }
    let Some(model_name) = first_ollama_model().await else {
        eprintln!("SKIP test_ollama_model_info: No Ollama models available");
        return;
    };

    let engine = OllamaInferenceEngine::new(model_name.clone());
    let info = engine
        .model_info()
        .await
        .expect("model_info() should not error");

    eprintln!("model_info() for {model_name}: {:?}", info);
    // model_info returns None if /api/show fails — that's acceptable
    if let Some(spec) = info {
        assert_eq!(spec.model_id, model_name);
        assert!(spec.context_window > 0);
    }
}

// ---------------------------------------------------------------------------
// Helper: resolve a real inference engine (Ollama → ministral-3b → skip)
//
// Returns None and prints a skip message if neither backend is available.
// The macro_name parameter is used only for the skip message.
// ---------------------------------------------------------------------------

/// Resolve a real inference engine for pipeline tests.
///
/// Priority:
/// 1. First available Ollama model (fast, no file required)
/// 2. Local ministral-3b GGUF (requires the file to be downloaded)
/// 3. Returns None — caller should skip the test
async fn resolve_engine(test_name: &str) -> Option<Arc<dyn ChatInferenceEngine>> {
    // 1. Try Ollama
    if ollama_running().await {
        if let Some(model_name) = first_ollama_model().await {
            eprintln!("[{test_name}] Using Ollama model: {model_name}");
            return Some(Arc::new(OllamaInferenceEngine::new(model_name)));
        }
    }

    // 2. Try local ministral-3b GGUF
    let gguf = match GgufModelManager::new() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("SKIP [{test_name}]: GgufModelManager::new() failed: {e}");
            return None;
        }
    };
    let model_path = match gguf.model_path("ministral-3b-q4km") {
        Ok(p) => p,
        Err(e) => {
            eprintln!("SKIP [{test_name}]: ministral-3b-q4km not in catalog: {e}");
            return None;
        }
    };
    if !model_path.exists() {
        eprintln!(
            "SKIP [{test_name}]: No inference backend available \
             (Ollama not running, ministral-3b not downloaded at {})",
            model_path.display()
        );
        return None;
    }
    let path_str = model_path.to_string_lossy().to_string();
    eprintln!("[{test_name}] Using local GGUF: {path_str}");
    match tokio::task::spawn_blocking(move || {
        LlamaChatInferenceEngine::load(&path_str, ChatConfig::default())
    })
    .await
    .expect("spawn_blocking")
    {
        Ok(engine) => Some(Arc::new(engine) as Arc<dyn ChatInferenceEngine>),
        Err(e) => {
            eprintln!("SKIP [{test_name}]: Failed to load ministral-3b: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal tool executor for pipeline tests — returns realistic stub results
// ---------------------------------------------------------------------------

/// Build a `dynamic_context` string that mimics what the app injects for the
/// Schema Creation skill: ENTITY TYPES + full skill guidance.
///
/// This makes tests as realistic as the live app — the model receives the same
/// guidance content it would get through the skill pipeline + prompt assembler.
fn schema_creation_context(entity_types: &str) -> String {
    // Pull the guidance content from the seeded skill definition
    let guidance = nodespace_agent::skill_pipeline::SkillPipeline::seed_skill_nodes()
        .into_iter()
        .find(|s| s.name == "Schema Creation")
        .and_then(|s| s.guidance_prompts.into_iter().next())
        .map(|g| g.content)
        .unwrap_or_default();

    format!(
        "{entity_types}\nACTIVE SKILL: Schema Creation\n\
         Define a new entity type or schema with custom fields, enums, and relationships. \
         Use when user says 'new type', 'node type', 'define fields', 'create schema', \
         or wants to design a new kind of entity like Project, Customer, or Invoice.\n\
         Focus on this skill's capabilities. Use only the tools provided.\n\n\
         {guidance}"
    )
}

struct StubToolExecutor {
    tools: Vec<ToolDefinition>,
    results: HashMap<String, serde_json::Value>,
}

/// A tool executor scoped to Schema Creation skill tools only: create_schema + get_node.
/// Mirrors what the skill pipeline produces after scoping tools to the matched skill's whitelist.
struct SchemaSkillToolExecutor {
    inner: StubToolExecutor,
}

impl StubToolExecutor {
    fn new() -> Self {
        let mut results = HashMap::new();
        results.insert(
            "search_nodes".to_string(),
            serde_json::json!({
                "count": 1,
                "nodes": [{"id": "task-abc123", "title": "Some thing to do", "type": "task",
                            "snippet": "Some thing to do", "status": "open"}]
            }),
        );
        results.insert(
            "search_semantic".to_string(),
            serde_json::json!({
                "count": 1,
                "nodes": [{"id": "task-abc123", "title": "Some thing to do", "type": "task"}]
            }),
        );
        results.insert(
            "get_node".to_string(),
            serde_json::json!({"id": "task-abc123", "title": "Some thing to do",
                               "type": "task", "status": "open"}),
        );
        results.insert(
            "update_node".to_string(),
            serde_json::json!({"id": "task-abc123", "updated": true}),
        );
        results.insert(
            "update_task_status".to_string(),
            serde_json::json!({"id": "task-abc123", "status": "in_progress", "updated": true}),
        );
        results.insert(
            "create_schema".to_string(),
            serde_json::json!({"id": "schema-proj1", "name": "Project", "created": true}),
        );
        results.insert(
            "create_node".to_string(),
            serde_json::json!({"id": "node-new1", "created": true}),
        );

        let tools = vec![
            ToolDefinition {
                name: "search_nodes".to_string(),
                description: "Search nodes by keyword and type filter".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "node_type": {"type": "string"}
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "search_semantic".to_string(),
                description: "Search nodes by semantic meaning".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "get_node".to_string(),
                description: "Get a node by ID".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"id": {"type": "string"}},
                    "required": ["id"]
                }),
            },
            ToolDefinition {
                name: "update_node".to_string(),
                description: "Update a node's fields".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "fields": {"type": "object"}
                    },
                    "required": ["id"]
                }),
            },
            ToolDefinition {
                name: "update_task_status".to_string(),
                description: "Update a task's status. Valid values: open, in_progress, done"
                    .to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "status": {"type": "string", "enum": ["open", "in_progress", "done"]}
                    },
                    "required": ["id", "status"]
                }),
            },
            ToolDefinition {
                name: "create_schema".to_string(),
                description: "Create a new node type (schema) with custom fields and relationships to other types".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "description": {"type": "string"},
                        "title_template": {"type": "string"},
                        "fields": {"type": "array"},
                        "relationships": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": {"type": "string"},
                                    "targetType": {"type": "string"},
                                    "direction": {"type": "string", "enum": ["in", "out"]},
                                    "cardinality": {"type": "string", "enum": ["one", "many"]}
                                },
                                "required": ["name", "targetType", "direction", "cardinality"]
                            }
                        }
                    },
                    "required": ["name"]
                }),
            },
            ToolDefinition {
                name: "create_node".to_string(),
                description: "Create a new node".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "node_type": {"type": "string"},
                        "content": {"type": "string"}
                    },
                    "required": ["node_type"]
                }),
            },
        ];

        Self { tools, results }
    }
}

#[async_trait]
impl AgentToolExecutor for StubToolExecutor {
    async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
        Ok(self.tools.clone())
    }

    async fn execute(&self, name: &str, _args: serde_json::Value) -> Result<ToolResult, ToolError> {
        let result = self
            .results
            .get(name)
            .cloned()
            .unwrap_or_else(|| serde_json::json!({"error": "unknown tool"}));
        let is_error = !self.results.contains_key(name);
        Ok(ToolResult {
            tool_call_id: format!("call_{name}"),
            name: name.to_string(),
            result,
            is_error,
        })
    }
}

impl SchemaSkillToolExecutor {
    fn new() -> Self {
        Self {
            inner: StubToolExecutor::new(),
        }
    }
}

#[async_trait]
impl AgentToolExecutor for SchemaSkillToolExecutor {
    async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
        // Return only the tools the Schema Creation skill whitelists
        let all = self.inner.available_tools().await?;
        Ok(all
            .into_iter()
            .filter(|t| t.name == "create_schema" || t.name == "get_node")
            .collect())
    }

    async fn execute(&self, name: &str, args: serde_json::Value) -> Result<ToolResult, ToolError> {
        self.inner.execute(name, args).await
    }
}

// ---------------------------------------------------------------------------
// Pipeline scenario tests — real model, full intent→tool flow
// ---------------------------------------------------------------------------

/// Helper: run one agent turn and return the tool names that were called.
async fn run_turn_get_tools(
    service: &LocalAgentService<dyn ChatInferenceEngine, dyn AgentToolExecutor>,
    session_id: &str,
    message: &str,
) -> Vec<String> {
    let (tools, _) = run_turn_get_tools_and_args(service, session_id, message).await;
    tools
}

/// Helper: run one agent turn and return tool names + their argument values.
async fn run_turn_get_tools_and_args(
    service: &LocalAgentService<dyn ChatInferenceEngine, dyn AgentToolExecutor>,
    session_id: &str,
    message: &str,
) -> (Vec<String>, Vec<serde_json::Value>) {
    let result = service
        .send_message(session_id, message, |_| {}, |_| {})
        .await
        .expect("send_message should succeed");
    eprintln!(
        "  response: {}",
        &result.response.chars().take(120).collect::<String>()
    );
    let tools: Vec<String> = result
        .tool_calls_made
        .iter()
        .map(|t| t.name.clone())
        .collect();
    let args: Vec<serde_json::Value> = result
        .tool_calls_made
        .iter()
        .map(|t| t.args.clone())
        .collect();
    eprintln!("  tools called: {:?}", tools);
    (tools, args)
}

/// Scenario: "Update the 'Some thing to do' task to in_progress"
///
/// The model should search for the task then call update_task_status.
#[tokio::test]
async fn test_pipeline_task_status_update() {
    let Some(engine) = resolve_engine("test_pipeline_task_status_update").await else {
        return;
    };

    let executor: Arc<dyn AgentToolExecutor> = Arc::new(StubToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    let tools = run_turn_get_tools(
        &service,
        &session_id,
        "Update the 'Some thing to do' task to in_progress",
    )
    .await;

    assert!(
        tools
            .iter()
            .any(|t| t == "update_task_status" || t == "update_node"),
        "Expected update_task_status or update_node to be called, got: {tools:?}"
    );
    // Should have searched before updating
    assert!(
        tools
            .iter()
            .any(|t| t == "search_nodes" || t == "search_semantic" || t == "get_node"),
        "Expected a search before update, got: {tools:?}"
    );
}

/// Scenario: "Create a 'Project' node type with fields we'd normally track on a project"
///
/// The model should call create_schema with a name and fields.
#[tokio::test]
async fn test_pipeline_schema_creation() {
    let Some(engine) = resolve_engine("test_pipeline_schema_creation").await else {
        return;
    };

    let executor: Arc<dyn AgentToolExecutor> = Arc::new(StubToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    let tools = run_turn_get_tools(
        &service,
        &session_id,
        "Create a 'Project' node type with fields we'd normally track on a project",
    )
    .await;

    assert!(
        tools.iter().any(|t| t == "create_schema"),
        "Expected create_schema to be called, got: {tools:?}"
    );
}

/// Scenario: "Create an Invoice node type... Use the Customer type for who it's billed to"
///
/// The model must call create_schema with a `relationships` entry targeting "customer".
/// This validates that the model correctly uses existing types in relationship definitions
/// rather than modeling cross-type references as plain text fields.
#[tokio::test]
async fn test_pipeline_schema_creation_with_relationship() {
    let Some(engine) = resolve_engine("test_pipeline_schema_creation_with_relationship").await
    else {
        return;
    };

    // Use schema-scoped executor (create_schema + get_node only) to mirror the
    // tool scoping the skill pipeline applies when Schema Creation skill matches.
    let executor: Arc<dyn AgentToolExecutor> = Arc::new(SchemaSkillToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    // Inject the full skill context: entity types + skill name/desc + guidance.
    // This mirrors what the app sends to Ollama after skill pipeline matching.
    let entity_types = "ENTITY TYPES:\n\
             - customer: Customer -- fields: name(text), email(text), phone(text), company(text)\n\
             - task: Task (core) -- fields: status(enum: open/in_progress/done)\n";
    service
        .set_session_context(&session_id, schema_creation_context(entity_types))
        .await;

    let (tools, args) = run_turn_get_tools_and_args(
        &service,
        &session_id,
        "Let's create an \"Invoice\" node type, with fields typical of what goes in an invoice. \
         Use the \"Customer\" type as for who the invoice is for field.",
    )
    .await;

    assert!(
        tools.iter().any(|t| t == "create_schema"),
        "Expected create_schema to be called, got: {tools:?}"
    );

    // Find the create_schema call args and verify a relationship to "customer" is present
    let schema_args = tools
        .iter()
        .zip(args.iter())
        .find(|(name, _)| *name == "create_schema")
        .map(|(_, a)| a)
        .expect("create_schema args not found");

    eprintln!("create_schema args: {}", serde_json::to_string_pretty(schema_args).unwrap());

    let relationships = schema_args.get("relationships").and_then(|r| r.as_array());
    assert!(
        relationships.is_some() && !relationships.unwrap().is_empty(),
        "Expected create_schema to include relationships, got args: {schema_args}"
    );

    let has_customer_rel = relationships
        .unwrap()
        .iter()
        .any(|r| r.get("targetType").and_then(|v| v.as_str()) == Some("customer"));
    assert!(
        has_customer_rel,
        "Expected a relationship with targetType 'customer', got relationships: {:?}",
        relationships
    );
}

/// Scenario: "Create a Project type with a one-to-many relationship to tasks"
///
/// Validates that the model correctly models a one→many relationship using
/// relationships (not an array field), with cardinality "many" targeting "task".
#[tokio::test]
async fn test_pipeline_schema_creation_project_task_relationship() {
    let Some(engine) =
        resolve_engine("test_pipeline_schema_creation_project_task_relationship").await
    else {
        return;
    };

    // Use schema-scoped executor (create_schema + get_node only) to mirror the
    // tool scoping the skill pipeline applies when Schema Creation skill matches.
    let executor: Arc<dyn AgentToolExecutor> = Arc::new(SchemaSkillToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    // Inject the full skill context: entity types + skill name/desc + guidance.
    let entity_types = "ENTITY TYPES:\n\
             - task: Task (core) -- fields: status(enum: open/in_progress/done)\n";
    service
        .set_session_context(&session_id, schema_creation_context(entity_types))
        .await;

    let (tools, args) = run_turn_get_tools_and_args(
        &service,
        &session_id,
        "Create a Project node type with typical project fields. \
         A project can have many tasks — model that as a relationship.",
    )
    .await;

    assert!(
        tools.iter().any(|t| t == "create_schema"),
        "Expected create_schema to be called, got: {tools:?}"
    );

    let schema_args = tools
        .iter()
        .zip(args.iter())
        .find(|(name, _)| *name == "create_schema")
        .map(|(_, a)| a)
        .expect("create_schema args not found");

    eprintln!(
        "create_schema args: {}",
        serde_json::to_string_pretty(schema_args).unwrap()
    );

    let relationships = schema_args.get("relationships").and_then(|r| r.as_array());
    assert!(
        relationships.is_some() && !relationships.unwrap().is_empty(),
        "Expected create_schema to include relationships, got args: {schema_args}"
    );

    let has_task_rel = relationships.unwrap().iter().any(|r| {
        r.get("targetType").and_then(|v| v.as_str()) == Some("task")
            && r.get("cardinality").and_then(|v| v.as_str()) == Some("many")
    });
    assert!(
        has_task_rel,
        "Expected a relationship with targetType 'task' and cardinality 'many', got: {:?}",
        relationships
    );
}

/// Scenario: multi-turn — two messages in the same session, each using tools.
///
/// This validates that the session survives across turns and conversation
/// history is preserved (the second message can reference the first).
#[tokio::test]
async fn test_pipeline_multi_turn_session_persistence() {
    let Some(engine) = resolve_engine("test_pipeline_multi_turn_session_persistence").await else {
        return;
    };

    let executor: Arc<dyn AgentToolExecutor> = Arc::new(StubToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    // Turn 1: task update
    eprintln!("--- Turn 1 ---");
    let tools1 = run_turn_get_tools(
        &service,
        &session_id,
        "Update the 'Some thing to do' task to in_progress",
    )
    .await;
    assert!(
        !tools1.is_empty(),
        "Turn 1 should have called at least one tool"
    );

    // Turn 2: schema creation — session must still be alive
    eprintln!("--- Turn 2 ---");
    let tools2 = run_turn_get_tools(
        &service,
        &session_id,
        "Now create a 'Project' node type with the fields we'd normally track in a project",
    )
    .await;
    assert!(
        tools2.iter().any(|t| t == "create_schema"),
        "Turn 2 expected create_schema, got: {tools2:?}"
    );
}
