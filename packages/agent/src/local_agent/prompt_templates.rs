//! Prompt templates for the local agent.
//!
//! Contains the tool-definition formatter and history summarization prompt used
//! by the ReAct loop.
//!
//! The system prompt itself is assembled exclusively by `PromptAssembler` from
//! graph-seeded prompt nodes; see [`crate::prompt_assembler`]. There is no inline
//! system-prompt duplication here — the only non-assembler path is the minimal
//! `EMERGENCY_FALLBACK_PROMPT` safety net in that module.

use crate::agent_types::ToolDefinition;

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

/// Build the prompt that turns the opening of a conversation into a short
/// title for it.
///
/// `conversation` is the rendered transcript (see `render_for_title` in
/// `packages/daemon/src/services/ai_chat_title.rs`, which caps how much of the
/// conversation is included). The instruction is phrased to suppress the
/// preamble a small local model otherwise volunteers ("Sure! Here's a
/// title:"), because the reply is used verbatim as the node's title — but the
/// caller still sanitises the result rather than trusting the model to comply.
pub fn title_generation_prompt(conversation: &str) -> String {
    format!(
        "Write a short title for the following conversation. \
         Use at most 6 words. Describe what the conversation is about. \
         Reply with the title alone — no quotes, no punctuation at the end, \
         no explanation, no preamble.\n\n\
         {conversation}"
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
    fn title_generation_prompt_includes_conversation() {
        let result = title_generation_prompt("user: how do I reset my password");
        assert!(result.contains("reset my password"));
        assert!(result.contains("title"));
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
