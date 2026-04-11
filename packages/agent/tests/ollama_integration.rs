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
use nodespace_agent::skill_pipeline::SkillPipeline;
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

// ===========================================================================
// Skill Pipeline Real Model E2E Tests (Issue #1057)
// ===========================================================================
//
// Each test resolves an inference engine using this fallback chain:
//   1. Try Ollama (first available model)
//   2. Fallback to local GGUF ministral-3b-q4km
//   3. Skip if neither is available
//
// Tests verify that each skill's guidance prompt + tool scoping leads the
// model to call one of the expected tools.

/// Resolve an inference engine: Ollama first, then local ministral-3b, then skip.
///
/// Returns `None` if no backend is available (caller should skip the test).
async fn resolve_engine(test_name: &str) -> Option<Arc<dyn ChatInferenceEngine>> {
    // Step 1: Try Ollama
    if ollama_running().await {
        if let Some(model) = first_ollama_model().await {
            let engine = OllamaInferenceEngine::new(model);
            return Some(Arc::new(engine) as Arc<dyn ChatInferenceEngine>);
        }
    }

    // Step 2: Fallback to local GGUF ministral-3b
    let gguf = match GgufModelManager::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("SKIP {test_name}: GgufModelManager::new() failed: {e}");
            return None;
        }
    };

    let model_path = match gguf.model_path("ministral-3b-q4km") {
        Ok(p) if p.exists() => p,
        _ => {
            eprintln!("SKIP {test_name}: No inference backend available (Ollama not running, ministral-3b not downloaded)");
            return None;
        }
    };

    let path_str = model_path.to_string_lossy().to_string();
    let engine = match tokio::task::spawn_blocking(move || {
        LlamaChatInferenceEngine::load(&path_str, ChatConfig::default())
    })
    .await
    .expect("spawn_blocking join error")
    {
        Ok(e) => e,
        Err(err) => {
            eprintln!("SKIP {test_name}: LlamaChatInferenceEngine::load failed: {err}");
            return None;
        }
    };

    Some(Arc::new(engine) as Arc<dyn ChatInferenceEngine>)
}

/// Build a scoped tool list for a skill (from the skill library).
fn tools_for_skill(skill_name: &str, all_tools: &[ToolDefinition]) -> Vec<ToolDefinition> {
    let seeds = SkillPipeline::seed_skill_nodes();
    let skill = seeds
        .iter()
        .find(|s| s.name == skill_name)
        .unwrap_or_else(|| panic!("Skill '{}' not found in seed skills", skill_name));

    let whitelist = &skill.tool_whitelist;
    all_tools
        .iter()
        .filter(|t| whitelist.contains(&t.name))
        .cloned()
        .collect()
}

/// All tool definitions as ToolDefinition stubs (names only, minimal schemas).
fn all_skill_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "search_semantic".into(),
            description: "Find nodes semantically related to a query".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "search_nodes".into(),
            description: "Search for nodes by keyword".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": { "query": { "type": "string" } },
                "required": ["query"]
            }),
        },
        ToolDefinition {
            name: "get_node".into(),
            description: "Get a node by ID".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "create_node".into(),
            description: "Create a new node".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "node_type": { "type": "string" }
                },
                "required": ["title", "node_type"]
            }),
        },
        ToolDefinition {
            name: "update_node".into(),
            description: "Update an existing node".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "update_task_status".into(),
            description: "Update a task's status".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "status": { "type": "string", "enum": ["open", "in_progress", "done", "cancelled"] }
                },
                "required": ["id", "status"]
            }),
        },
        ToolDefinition {
            name: "create_schema".into(),
            description: "Create a new entity type schema".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: "create_relationship".into(),
            description: "Create a relationship between two nodes".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "from_id": { "type": "string" },
                    "to_id": { "type": "string" },
                    "relationship_type": { "type": "string" }
                },
                "required": ["from_id", "to_id", "relationship_type"]
            }),
        },
        ToolDefinition {
            name: "get_related_nodes".into(),
            description: "Get nodes related to a given node".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "delete_node".into(),
            description: "Delete a node from the knowledge graph".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: "create_nodes_from_markdown".into(),
            description: "Import a markdown document and create a hierarchy of nodes".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": { "markdown": { "type": "string" } },
                "required": ["markdown"]
            }),
        },
    ]
}

/// Run a single-turn inference with a scoped tool list, return tool call names.
async fn run_skill_inference(
    engine: &dyn ChatInferenceEngine,
    user_message: &str,
    skill_description: &str,
    tools: Vec<ToolDefinition>,
) -> Vec<String> {
    let system = format!(
        "You are a helpful assistant managing a knowledge graph.\n\nACTIVE SKILL: {skill_description}\n\nUse the available tools to complete the user's request."
    );

    let request = InferenceRequest {
        messages: vec![
            ChatMessage {
                role: Role::System,
                content: system,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: Role::User,
                content: user_message.to_string(),
                tool_call_id: None,
                name: None,
            },
        ],
        tools: Some(tools),
        temperature: Some(0.0),
        max_tokens: Some(200),
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
        .expect("generate() should succeed");

    let collected = chunks.lock().unwrap();
    let mut tool_calls = Vec::new();
    for chunk in collected.iter() {
        if let StreamingChunk::ToolCallStart { name, .. } = chunk {
            tool_calls.push(name.clone());
        }
    }
    tool_calls
}

#[tokio::test]
async fn test_skill_pipeline_research_real_model() {
    let test_name = "test_skill_pipeline_research_real_model";
    let Some(engine) = resolve_engine(test_name).await else {
        return;
    };

    let all_tools = all_skill_tools();
    let tools = tools_for_skill("Research & Search", &all_tools);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    eprintln!("Research tools offered: {:?}", tool_names);

    let tool_calls = run_skill_inference(
        engine.as_ref(),
        "What do I know about machine learning?",
        "Search and explore the knowledge graph to find relevant information.",
        tools,
    )
    .await;

    eprintln!("{test_name}: tool calls = {:?}", tool_calls);

    assert!(
        tool_calls
            .iter()
            .any(|c| c == "search_semantic" || c == "search_nodes"),
        "Research skill must call a search tool, got: {tool_calls:?}"
    );
    for call in &tool_calls {
        assert!(
            call == "search_semantic" || call == "search_nodes" || call == "get_node",
            "Research skill should only call search/get tools, got: {call}"
        );
    }
}

#[tokio::test]
async fn test_skill_pipeline_node_creation_real_model() {
    let test_name = "test_skill_pipeline_node_creation_real_model";
    let Some(engine) = resolve_engine(test_name).await else {
        return;
    };

    let all_tools = all_skill_tools();
    let tools = tools_for_skill("Node Creation", &all_tools);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    eprintln!("Node Creation tools offered: {:?}", tool_names);

    let tool_calls = run_skill_inference(
        engine.as_ref(),
        "Create a new task node called 'Review quarterly report'",
        "Create new instances of existing node types.",
        tools,
    )
    .await;

    eprintln!("{test_name}: tool calls = {:?}", tool_calls);

    assert!(
        tool_calls.iter().any(|c| c == "create_node"),
        "Node Creation skill must call create_node, got: {tool_calls:?}"
    );
    for call in &tool_calls {
        assert!(
            call == "create_node" || call == "get_node",
            "Node Creation skill should only call create_node/get_node, got: {call}"
        );
    }
}

#[tokio::test]
async fn test_skill_pipeline_schema_creation_real_model() {
    let test_name = "test_skill_pipeline_schema_creation_real_model";
    let Some(engine) = resolve_engine(test_name).await else {
        return;
    };

    let all_tools = all_skill_tools();
    let tools = tools_for_skill("Schema Creation", &all_tools);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    eprintln!("Schema Creation tools offered: {:?}", tool_names);

    let tool_calls = run_skill_inference(
        engine.as_ref(),
        "Create a new type called Project with fields for status and deadline",
        "Define new entity types with custom fields.",
        tools,
    )
    .await;

    eprintln!("{test_name}: tool calls = {:?}", tool_calls);

    for call in &tool_calls {
        assert!(
            call == "create_schema" || call == "get_node",
            "Schema Creation skill should only call create_schema/get_node, got: {call}"
        );
    }
}

#[tokio::test]
async fn test_skill_pipeline_graph_editing_real_model() {
    let test_name = "test_skill_pipeline_graph_editing_real_model";
    let Some(engine) = resolve_engine(test_name).await else {
        return;
    };

    let all_tools = all_skill_tools();
    let tools = tools_for_skill("Graph Editing", &all_tools);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    eprintln!("Graph Editing tools offered: {:?}", tool_names);

    let tool_calls = run_skill_inference(
        engine.as_ref(),
        "Find the 'Architecture Notes' node and update its title to 'System Architecture'",
        "Modify existing nodes in the knowledge graph.",
        tools,
    )
    .await;

    eprintln!("{test_name}: tool calls = {:?}", tool_calls);

    assert!(
        tool_calls.iter().any(|c| c == "update_node"),
        "Graph Editing skill must call update_node, got: {tool_calls:?}"
    );
    for call in &tool_calls {
        assert!(
            call == "update_node"
                || call == "update_task_status"
                || call == "get_node"
                || call == "search_nodes",
            "Graph Editing skill should only call update/get/search tools, got: {call}"
        );
    }
}

#[tokio::test]
async fn test_skill_pipeline_relationship_management_real_model() {
    let test_name = "test_skill_pipeline_relationship_management_real_model";
    let Some(engine) = resolve_engine(test_name).await else {
        return;
    };

    let all_tools = all_skill_tools();
    let tools = tools_for_skill("Relationship Management", &all_tools);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    eprintln!("Relationship Management tools offered: {:?}", tool_names);

    let tool_calls = run_skill_inference(
        engine.as_ref(),
        "Connect node invoice-001 to node customer-456 with a 'belongs_to' relationship",
        "Create connections between nodes.",
        tools,
    )
    .await;

    eprintln!("{test_name}: tool calls = {:?}", tool_calls);

    for call in &tool_calls {
        assert!(
            call == "create_relationship" || call == "get_related_nodes" || call == "get_node",
            "Relationship Management skill should only call relationship tools, got: {call}"
        );
    }
}

#[tokio::test]
async fn test_skill_pipeline_node_deletion_real_model() {
    let test_name = "test_skill_pipeline_node_deletion_real_model";
    let Some(engine) = resolve_engine(test_name).await else {
        return;
    };

    let all_tools = all_skill_tools();
    let tools = tools_for_skill("Node Deletion", &all_tools);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    eprintln!("Node Deletion tools offered: {:?}", tool_names);

    let tool_calls = run_skill_inference(
        engine.as_ref(),
        "Delete node old-meeting-notes-789",
        "Delete nodes from the knowledge graph.",
        tools,
    )
    .await;

    eprintln!("{test_name}: tool calls = {:?}", tool_calls);

    assert!(
        tool_calls.iter().any(|c| c == "delete_node"),
        "Node Deletion skill must call delete_node, got: {tool_calls:?}"
    );
    for call in &tool_calls {
        assert!(
            call == "delete_node" || call == "get_node",
            "Node Deletion skill should only call delete_node/get_node, got: {call}"
        );
    }
}

#[tokio::test]
async fn test_skill_pipeline_bulk_import_real_model() {
    let test_name = "test_skill_pipeline_bulk_import_real_model";
    let Some(engine) = resolve_engine(test_name).await else {
        return;
    };

    let all_tools = all_skill_tools();
    let tools = tools_for_skill("Bulk Import", &all_tools);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    eprintln!("Bulk Import tools offered: {:?}", tool_names);

    let tool_calls = run_skill_inference(
        engine.as_ref(),
        "Import the following markdown: '# My Notes\\n\\nThis is a note.'",
        "Import documents and create node hierarchies from markdown.",
        tools,
    )
    .await;

    eprintln!("{test_name}: tool calls = {:?}", tool_calls);

    for call in &tool_calls {
        assert!(
            call == "create_nodes_from_markdown",
            "Bulk Import skill should only call create_nodes_from_markdown, got: {call}"
        );
    }
}

#[tokio::test]
async fn test_skill_pipeline_organization_real_model() {
    let test_name = "test_skill_pipeline_organization_real_model";
    let Some(engine) = resolve_engine(test_name).await else {
        return;
    };

    let all_tools = all_skill_tools();
    let tools = tools_for_skill("Organization", &all_tools);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    eprintln!("Organization tools offered: {:?}", tool_names);

    let tool_calls = run_skill_inference(
        engine.as_ref(),
        "Add node project-123 to the collection 'Active Projects'",
        "Organize nodes into collections and categories.",
        tools,
    )
    .await;

    eprintln!("{test_name}: tool calls = {:?}", tool_calls);

    for call in &tool_calls {
        assert!(
            call == "create_relationship" || call == "get_node",
            "Organization skill should only call create_relationship/get_node, got: {call}"
        );
    }
}

// ---------------------------------------------------------------------------
// Minimal tool executor for pipeline tests — returns realistic stub results
// ---------------------------------------------------------------------------

/// Build the full system prompt that the Schema Creation skill would produce.
///
/// Mirrors what the live app sends to Ollama:
///   1. Base prompt from `PromptAssembler::seed_prompt_nodes()` (with entity types
///      rendered into the Workspace Context Template)
///   2. Active skill header + description (from `SkillPipeline::seed_skill_nodes()`)
///   3. Skill guidance content
///
/// Using the real seed sources means any change to prompt content or skill guidance
/// is automatically reflected in tests — no manual sync needed.
fn schema_creation_context(entity_types: &str) -> String {
    use nodespace_agent::prompt_assembler::PromptAssembler;

    // 1. Base prompt with entity types injected into the workspace context slot.
    let base = PromptAssembler::assemble_static(entity_types, None);

    // 2. Skill name, description, and guidance from the seeded skill definition.
    let skill = nodespace_agent::skill_pipeline::SkillPipeline::seed_skill_nodes()
        .into_iter()
        .find(|s| s.name == "Schema Creation");

    let (skill_desc, guidance) = if let Some(s) = skill {
        let desc = s.description.clone();
        let g = s
            .guidance_prompts
            .into_iter()
            .next()
            .map(|g| g.content)
            .unwrap_or_default();
        (desc, g)
    } else {
        (String::new(), String::new())
    };

    format!(
        "{base}\n\nACTIVE SKILL: Schema Creation\n\
         {skill_desc}\n\
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
        results.insert(
            "create_relationship".to_string(),
            serde_json::json!({"created": true}),
        );
        results.insert(
            "get_related_nodes".to_string(),
            serde_json::json!({
                "count": 1,
                "nodes": [{"id": "note-xyz", "title": "Architecture Notes", "type": "text"}]
            }),
        );
        results.insert(
            "delete_node".to_string(),
            serde_json::json!({"deleted": true}),
        );

        let tools = vec![
            ToolDefinition {
                name: "search_nodes".to_string(),
                description: "Search nodes by title keyword and/or filter by type and properties. Pass query=\"\" to skip title filter (e.g. to list all tasks). Use node_type to filter by type. Use filters for property key-value pairs (e.g. filters={\"status\":\"open\"}).".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "Keyword or phrase to search for in node titles. Pass empty string to skip title filter."},
                        "node_type": {"type": "string"},
                        "filters": {
                            "type": "object",
                            "description": "Property filters as key-value pairs, e.g. {\"status\": \"open\"}.",
                            "additionalProperties": {"type": "string"}
                        }
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
            ToolDefinition {
                name: "create_relationship".to_string(),
                description: "Create a relationship between two nodes".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "source_id": {"type": "string"},
                        "target_id": {"type": "string"},
                        "relationship_type": {"type": "string"}
                    },
                    "required": ["source_id", "target_id", "relationship_type"]
                }),
            },
            ToolDefinition {
                name: "get_related_nodes".to_string(),
                description: "Get nodes related to a given node. Use when asked what is connected to a node.".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "relationship_type": {"type": "string"}
                    },
                    "required": ["id"]
                }),
            },
            ToolDefinition {
                name: "delete_node".to_string(),
                description: "Delete a node from the knowledge graph by its ID".to_string(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"id": {"type": "string"}},
                    "required": ["id"]
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
        .set_system_prompt(&session_id, schema_creation_context(entity_types))
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

    eprintln!(
        "create_schema args: {}",
        serde_json::to_string_pretty(schema_args).unwrap()
    );

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
        .set_system_prompt(&session_id, schema_creation_context(entity_types))
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

/// Scenario: "find the task called 'Review quarterly report'"
///
/// The model should call search_nodes (keyword/title match), not search_semantic.
/// This is the canonical case for search_nodes: the user knows the exact name.
#[tokio::test]
async fn test_pipeline_search_nodes_keyword() {
    let Some(engine) = resolve_engine("test_pipeline_search_nodes_keyword").await else {
        return;
    };

    let executor: Arc<dyn AgentToolExecutor> = Arc::new(StubToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    let tools = run_turn_get_tools(
        &service,
        &session_id,
        "Find the task called 'Review quarterly report'",
    )
    .await;

    eprintln!("test_pipeline_search_nodes_keyword: tools = {tools:?}");

    assert!(
        tools.iter().any(|t| t == "search_nodes"),
        "Expected search_nodes for exact-name lookup, got: {tools:?}"
    );
}

/// Scenario: "show me all nodes connected to the Architecture Notes node"
///
/// The model should search for the node then call get_related_nodes.
#[tokio::test]
async fn test_pipeline_get_related_nodes() {
    let Some(engine) = resolve_engine("test_pipeline_get_related_nodes").await else {
        return;
    };

    let executor: Arc<dyn AgentToolExecutor> = Arc::new(StubToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    let tools = run_turn_get_tools(
        &service,
        &session_id,
        "Show me all nodes connected to the 'Architecture Notes' node",
    )
    .await;

    eprintln!("test_pipeline_get_related_nodes: tools = {tools:?}");

    assert!(
        tools.iter().any(|t| t == "get_related_nodes"),
        "Expected get_related_nodes, got: {tools:?}"
    );
}

/// Scenario: "update the title of the 'Architecture Notes' node to 'System Architecture'"
///
/// The model should search for the node then call update_node (not update_task_status,
/// which is only for task status changes).
#[tokio::test]
async fn test_pipeline_update_node_content() {
    let Some(engine) = resolve_engine("test_pipeline_update_node_content").await else {
        return;
    };

    let executor: Arc<dyn AgentToolExecutor> = Arc::new(StubToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    let tools = run_turn_get_tools(
        &service,
        &session_id,
        "Update the title of the 'Architecture Notes' node to 'System Architecture'",
    )
    .await;

    eprintln!("test_pipeline_update_node_content: tools = {tools:?}");

    assert!(
        tools.iter().any(|t| t == "update_node"),
        "Expected update_node for title change, got: {tools:?}"
    );
}

/// Scenario: "show me the full content of node task-abc123"
///
/// The model is given an explicit node ID and should call get_node directly.
#[tokio::test]
async fn test_pipeline_get_node_by_id() {
    let Some(engine) = resolve_engine("test_pipeline_get_node_by_id").await else {
        return;
    };

    let executor: Arc<dyn AgentToolExecutor> = Arc::new(StubToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    let tools = run_turn_get_tools(
        &service,
        &session_id,
        "Show me the full content of node task-abc123",
    )
    .await;

    eprintln!("test_pipeline_get_node_by_id: tools = {tools:?}");

    assert!(
        tools.iter().any(|t| t == "get_node"),
        "Expected get_node for explicit ID lookup, got: {tools:?}"
    );
}

/// Scenario: create a schema with a title_template that references a field.
///
/// The model MUST include all fields referenced in title_template in the fields array.
/// This was a recurring bug where the model generated title_template: "{name} ({status})"
/// but forgot to include "name" in fields, causing validation failures and retry loops.
#[tokio::test]
async fn test_pipeline_schema_creation_title_template_fields() {
    let Some(engine) = resolve_engine("test_pipeline_schema_creation_title_template_fields").await
    else {
        return;
    };

    let executor: Arc<dyn AgentToolExecutor> = Arc::new(SchemaSkillToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    service
        .set_system_prompt(&session_id, schema_creation_context(""))
        .await;

    let (tools, args) = run_turn_get_tools_and_args(
        &service,
        &session_id,
        "Create a 'Campaign' schema with a name and status field. \
         Use title_template to show the title as '{name} ({status})'.",
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

    let template = schema_args
        .get("title_template")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !template.is_empty() {
        // Extract {field} tokens from the template
        let fields = schema_args
            .get("fields")
            .and_then(|f| f.as_array())
            .cloned()
            .unwrap_or_default();
        let field_names: Vec<&str> = fields
            .iter()
            .filter_map(|f| f.get("name").and_then(|n| n.as_str()))
            .collect();

        // Find all {token} references in the template
        let mut i = 0;
        let bytes = template.as_bytes();
        while i < bytes.len() {
            if bytes[i] == b'{' {
                if let Some(end) = bytes[i + 1..].iter().position(|&c| c == b'}') {
                    let token = &template[i + 1..i + 1 + end];
                    assert!(
                        field_names.contains(&token),
                        "title_template references '{{{}}}' but '{}' is not in fields array. \
                         fields: {:?}, template: {}",
                        token,
                        token,
                        field_names,
                        template
                    );
                    i += 1 + end + 1;
                    continue;
                }
            }
            i += 1;
        }
    }
}

/// Scenario: "find all my open tasks"
///
/// The model should call search_nodes with node_type="task" (not search_semantic)
/// to list tasks filtered by type.
#[tokio::test]
async fn test_pipeline_search_nodes_with_type_filter() {
    let Some(engine) = resolve_engine("test_pipeline_search_nodes_with_type_filter").await else {
        return;
    };

    let executor: Arc<dyn AgentToolExecutor> = Arc::new(StubToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    let tools = run_turn_get_tools(&service, &session_id, "Find all my open tasks").await;

    eprintln!("test_pipeline_search_nodes_with_type_filter: tools = {tools:?}");

    assert!(
        tools.iter().any(|t| t == "search_nodes"),
        "Expected search_nodes for listing tasks by type, got: {tools:?}"
    );
}

/// Scenario: "delete the 'Some thing to do' task"
///
/// The model should search for the task then call delete_node.
#[tokio::test]
async fn test_pipeline_delete_node() {
    let Some(engine) = resolve_engine("test_pipeline_delete_node").await else {
        return;
    };

    let executor: Arc<dyn AgentToolExecutor> = Arc::new(StubToolExecutor::new());
    let service = LocalAgentService::new(engine, executor, None);
    let session_id = service.create_session(None).await;

    let tools = run_turn_get_tools(
        &service,
        &session_id,
        "Delete the task called 'Some thing to do'",
    )
    .await;

    eprintln!("test_pipeline_delete_node: tools = {tools:?}");

    assert!(
        tools.iter().any(|t| t == "delete_node"),
        "Expected delete_node, got: {tools:?}"
    );
}
