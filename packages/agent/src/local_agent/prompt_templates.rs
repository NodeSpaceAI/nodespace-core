//! Prompt templates for the local agent.
//!
//! Contains the fallback system prompt, tool-definition formatter, and history
//! summarization prompt used by the ReAct loop.
//!
//! Note: The primary prompt assembly path uses `PromptAssembler` which reads
//! prompt content exclusively from graph nodes. The `fallback_system_prompt()`
//! here is only for use when `PromptAssembler` is not available (e.g., the
//! agent loop has not yet been wired to accept a `PromptAssembler` instance).

use crate::agent_types::ToolDefinition;

/// Fallback system prompt for the local agent.
///
/// Only used when `PromptAssembler` is not available (e.g., the agent loop
/// has not yet been wired to accept a `PromptAssembler` instance). The
/// primary prompt path reads all content from graph nodes via `PromptAssembler`.
///
/// `dynamic_context` is a pre-formatted string describing the workspace's
/// entity types, collections, and active playbooks (built by
/// `context_ops::build_workspace_context` + `format_for_prompt`).
pub fn fallback_system_prompt(dynamic_context: &str) -> String {
    let ctx_block = if dynamic_context.is_empty() {
        String::new()
    } else {
        format!("\n{dynamic_context}\n")
    };

    format!(
        "You are NodeSpace's built-in assistant. You help users work with their \
         knowledge graph — creating, finding, updating, and connecting nodes.\
         {ctx_block}\n\
         TOOL STRATEGY:\n\
         - ALWAYS search first before updating or getting a node. NEVER use placeholder IDs like \"abc-123\".\n\
         - To find nodes by meaning/topic: use search_semantic (natural language query)\n\
         - To find nodes by exact fields: use search_nodes (keyword + type filter)\n\
         - To get full node details: use get_node with the ID from search results\n\
         - To update a task status: search for the task first, then use update_task_status with the real ID\n\
         - To create a new entity type: use create_schema (not create_node)\n\
         - To create an instance of an existing type: use create_node with node_type matching the schema ID\n\
         - To connect nodes: use create_relationship with relationship names from the schemas above\n\
         - Tool call arguments must be valid JSON. Do NOT include comments (#) in JSON.\n\n\
         RESPONSE RULES:\n\
         - When the user's intent is clear, call the tool immediately — do NOT describe your plan first.\n\
         - Do NOT narrate what you are about to do (\"I'll now create...\", \"Let me search...\", \"Next I will...\").\n\
         - Do NOT show intermediate reasoning or self-corrections before a tool call.\n\
         - After tool results: summarize in natural language. NEVER paste raw JSON as your response.\n\
         - Reference nodes with bare URI: nodespace://abc-123 (no markdown links, no backticks)\n\
         - Enum values in tool calls: use exact schema values (\"done\", \"in_progress\"). In responses to user: use friendly labels (\"Done\", \"In Progress\").\n\
         - When listing nodes: **Title** (nodespace://id) — brief description\n\
         - When reporting search results: \"Found N nodes...\" then list top results\n\
         - If tool returns empty results: say so clearly. Do NOT retry the same query.\n\
         - Keep responses concise — under 3 sentences unless user asks for detail.\n\n\
         TOOL CALL FORMAT:\n\
         - Pass arguments flat. Do NOT nest under \"properties\" or \"arguments\".\n\
         - Use the exact field names shown in the schema definitions above."
    )
}

/// Format tool definitions into the text block appended to the system prompt.
///
/// Produces a compact representation that fits the context budget of a small
/// local model (~2k tokens reserved for system prompt + tools).
pub fn format_tool_definitions(tools: &[ToolDefinition]) -> String {
    if tools.is_empty() {
        return String::new();
    }

    let mut out = String::from("\n\nAvailable tools:\n");
    for tool in tools {
        out.push_str(&format!(
            "- {}: {}\n  Parameters: {}\n",
            tool.name,
            tool.description,
            serde_json::to_string(&tool.parameters_schema).unwrap_or_default(),
        ));
    }
    out
}

/// Build the prompt used to summarize older conversation turns.
///
/// The caller inserts the older messages as a block between the instruction
/// and the model's response.
pub fn summarization_prompt(older_messages: &str) -> String {
    format!(
        "Summarize the following conversation history into key facts and context. \
         Preserve node IDs, tool results, and user preferences. Be concise.\n\n\
         {older_messages}"
    )
}

/// Format a tool result as JSON for the conversation history.
///
/// The content is serialized as JSON so that the nlp-engine can parse it and
/// wrap it in Mistral's `[TOOL_RESULTS]` tags during template application.
pub fn format_tool_result(_name: &str, result: &serde_json::Value, is_error: bool) -> String {
    if is_error {
        serde_json::to_string(&serde_json::json!({"error": result})).unwrap_or_default()
    } else {
        serde_json::to_string(result).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fallback_system_prompt_includes_context() {
        let ctx = "ENTITY TYPES:\n- customer: Customer\n";
        let prompt = fallback_system_prompt(ctx);
        assert!(prompt.contains("NodeSpace"));
        assert!(prompt.contains("ENTITY TYPES:"));
        assert!(prompt.contains("customer: Customer"));
        assert!(prompt.contains("TOOL STRATEGY:"));
        assert!(prompt.contains("RESPONSE RULES:"));
    }

    #[test]
    fn fallback_system_prompt_empty_context() {
        let prompt = fallback_system_prompt("");
        assert!(prompt.contains("NodeSpace"));
        assert!(prompt.contains("TOOL STRATEGY:"));
        // No double newlines from empty context
        assert!(!prompt.contains("\n\n\n\n"));
    }

    #[test]
    fn fallback_system_prompt_per_turn_refresh() {
        // Simulate dynamic context being refreshed per turn
        let workspace_context_1 =
            "ENTITY TYPES:\n- task: Task (core) -- fields: status(enum: open/done)\n";
        let prompt_1 = fallback_system_prompt(workspace_context_1);

        assert!(prompt_1.contains("NodeSpace"));
        assert!(prompt_1.contains("ENTITY TYPES:"));
        assert!(prompt_1.contains("task: Task"));
        assert!(prompt_1.contains("status(enum: open/done)"));

        // Simulate a new schema added and context refreshed
        let workspace_context_2 = "ENTITY TYPES:\n\
            - task: Task (core) -- fields: status(enum: open/done)\n\
            - customer: Customer -- fields: name(text), email(text)\n";
        let prompt_2 = fallback_system_prompt(workspace_context_2);

        // Should include both old and new types
        assert!(prompt_2.contains("task: Task"));
        assert!(prompt_2.contains("customer: Customer"));
        assert!(prompt_2.contains("email(text)"));
    }

    #[test]
    fn format_tool_definitions_empty() {
        assert!(format_tool_definitions(&[]).is_empty());
    }

    #[test]
    fn format_tool_definitions_single_tool() {
        let tools = vec![ToolDefinition {
            name: "search_nodes".into(),
            description: "Search for nodes".into(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
        }];
        let result = format_tool_definitions(&tools);
        assert!(result.contains("search_nodes"));
        assert!(result.contains("Search for nodes"));
        assert!(result.contains("query"));
    }

    #[test]
    fn format_tool_definitions_multiple() {
        let tools = vec![
            ToolDefinition {
                name: "tool_a".into(),
                description: "First tool".into(),
                parameters_schema: json!({"type": "object"}),
            },
            ToolDefinition {
                name: "tool_b".into(),
                description: "Second tool".into(),
                parameters_schema: json!({"type": "object"}),
            },
        ];
        let result = format_tool_definitions(&tools);
        assert!(result.contains("tool_a"));
        assert!(result.contains("tool_b"));
    }

    #[test]
    fn summarization_prompt_includes_messages() {
        let result = summarization_prompt("User asked about billing architecture");
        assert!(result.contains("billing architecture"));
        assert!(result.contains("Summarize"));
    }

    #[test]
    fn format_tool_result_success() {
        let result = format_tool_result("search_nodes", &json!({"count": 3}), false);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["count"], 3);
    }

    #[test]
    fn format_tool_result_error() {
        let result = format_tool_result("get_node", &json!("not found"), true);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["error"], "not found");
    }
}
