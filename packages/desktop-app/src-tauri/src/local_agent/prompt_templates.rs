//! Prompt templates for the local agent.
//!
//! Contains the system prompt, tool-definition formatter, and history
//! summarization prompt used by the ReAct loop.

use crate::agent_types::ToolDefinition;

/// Build the system prompt for the local agent.
///
/// Describes the agent's role and available tool-calling conventions.
pub fn system_prompt() -> String {
    "You are a knowledge graph assistant for NodeSpace. You help users organize, \
     search, and manage their knowledge.\n\n\
     When you need information, use the available tools.\n\n\
     NODE REFERENCES: When referencing nodes, always use the bare nodespace:// URI \
     format (e.g. nodespace://abc-123). Do NOT wrap URIs in markdown links or \
     backticks — the client auto-links them. The URI format is nodespace://{node_id}.\n\n\
     DISPLAY FORMATTING: When showing enum values (e.g. node_type, status), \
     display them in Title Case (e.g. \"In Progress\" not \"in_progress\").\n\n\
     Respond concisely. If the user's question can be answered directly, \
     do so without calling tools."
        .to_string()
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

/// Format a tool result into a human-readable string for the conversation history.
pub fn format_tool_result(name: &str, result: &serde_json::Value, is_error: bool) -> String {
    if is_error {
        format!("Tool: {name}\nError: {result}")
    } else {
        format!("Tool: {name}\nResult: {result}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn system_prompt_is_non_empty() {
        let prompt = system_prompt();
        assert!(!prompt.is_empty());
        assert!(prompt.contains("NodeSpace"));
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
        assert!(result.contains("Tool: search_nodes"));
        assert!(result.contains("Result:"));
    }

    #[test]
    fn format_tool_result_error() {
        let result = format_tool_result("get_node", &json!({"error": "not found"}), true);
        assert!(result.contains("Tool: get_node"));
        assert!(result.contains("Error:"));
    }
}
