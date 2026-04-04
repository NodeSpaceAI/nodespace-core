/// Tool-call parser for Mistral raw GGUF format.
///
/// Mistral models (when loaded as raw GGUF without API wrapping) emit tool calls
/// in a non-standard format:
///
/// ```text
/// [TOOL_CALLS]function_name[ARGS]{"param": "value"}
/// ```
///
/// Multiple calls can appear in one response:
///
/// ```text
/// [TOOL_CALLS]search_nodes[ARGS]{"query": "embeddings"}[TOOL_CALLS]search_nodes[ARGS]{"query": "vector search"}
/// ```
///
/// This module provides both a complete-text parser and a streaming parser that
/// handles partial sentinels split across token boundaries.
/// Sentinel markers in Mistral tool-call format.
const TOOL_CALLS_SENTINEL: &str = "[TOOL_CALLS]";
const ARGS_SENTINEL: &str = "[ARGS]";

/// A single parsed tool call extracted from model output.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedToolCall {
    /// Name of the tool the model wants to invoke.
    pub name: String,
    /// Parsed JSON arguments for the tool.
    pub args: serde_json::Value,
}

/// Result of attempting to parse tool calls from complete text.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseResult {
    /// No tool-call sentinel found; the text is plain assistant output.
    PlainText(String),
    /// One or more tool calls were found.
    ToolCalls(Vec<ParsedToolCall>),
    /// The sentinel was found but the format was invalid.
    Error(String),
}

/// Parse tool calls from a complete response string.
///
/// Returns `ParseResult::PlainText` if no `[TOOL_CALLS]` sentinel is found,
/// `ParseResult::ToolCalls` for successfully parsed calls, or
/// `ParseResult::Error` for malformed tool-call output.
pub fn parse_tool_calls(text: &str) -> ParseResult {
    if !text.contains(TOOL_CALLS_SENTINEL) {
        return ParseResult::PlainText(text.to_string());
    }

    let mut calls = Vec::new();
    let mut remaining = text;

    // Skip any text before the first [TOOL_CALLS] sentinel
    if let Some(idx) = remaining.find(TOOL_CALLS_SENTINEL) {
        remaining = &remaining[idx..];
    }

    while let Some(tc_start) = remaining.find(TOOL_CALLS_SENTINEL) {
        let after_sentinel = &remaining[tc_start + TOOL_CALLS_SENTINEL.len()..];

        // Find [ARGS] sentinel to separate function name from arguments
        let args_pos = match after_sentinel.find(ARGS_SENTINEL) {
            Some(pos) => pos,
            None => {
                return ParseResult::Error(format!(
                    "Missing [ARGS] sentinel after [TOOL_CALLS] at position {}",
                    tc_start
                ));
            }
        };

        let function_name = after_sentinel[..args_pos].trim().to_string();
        if function_name.is_empty() {
            return ParseResult::Error("Empty function name after [TOOL_CALLS]".to_string());
        }

        let after_args = &after_sentinel[args_pos + ARGS_SENTINEL.len()..];

        // Extract JSON: everything from here until the next [TOOL_CALLS] or end of string
        let json_end = after_args
            .find(TOOL_CALLS_SENTINEL)
            .unwrap_or(after_args.len());
        let json_str = after_args[..json_end].trim();

        if json_str.is_empty() {
            return ParseResult::Error(format!(
                "Empty arguments for tool call '{}'",
                function_name
            ));
        }

        match serde_json::from_str::<serde_json::Value>(json_str) {
            Ok(args) => {
                calls.push(ParsedToolCall {
                    name: function_name,
                    args,
                });
            }
            Err(e) => {
                return ParseResult::Error(format!(
                    "Invalid JSON for tool call '{}': {} (raw: {:?})",
                    calls.last().map_or("unknown", |c| c.name.as_str()),
                    e,
                    json_str
                ));
            }
        }

        // Advance past this tool call
        remaining = &after_args[json_end..];
    }

    if calls.is_empty() {
        ParseResult::Error("Found [TOOL_CALLS] sentinel but parsed zero tool calls".to_string())
    } else {
        ParseResult::ToolCalls(calls)
    }
}

// ---------------------------------------------------------------------------
// Streaming parser
// ---------------------------------------------------------------------------

/// State machine for parsing tool calls from a stream of tokens.
///
/// Tokens arrive one at a time and may split sentinels across boundaries.
/// The parser accumulates text and detects when the `[TOOL_CALLS]` sentinel
/// appears, switching into tool-call parsing mode.
#[derive(Debug)]
pub struct StreamingToolCallParser {
    /// Accumulated raw output from the model.
    buffer: String,
    /// Whether we have detected the `[TOOL_CALLS]` sentinel in the stream.
    in_tool_call_mode: bool,
    /// Text that was emitted as plain tokens before the sentinel was detected.
    /// If the sentinel never appears, all text is plain output.
    plain_prefix: String,
}

/// Events emitted by the streaming parser as tokens arrive.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// A plain text token that should be forwarded to the user.
    TextToken(String),
    /// A complete tool call was parsed from the accumulated buffer.
    ToolCall(ParsedToolCall),
    /// The buffer contains a partial sentinel that may complete with more tokens.
    /// The caller should NOT emit these characters as text yet.
    Buffering,
    /// Parsing completed (end of stream). Contains any remaining tool calls
    /// or an error if the buffer was malformed.
    Finished(ParseResult),
}

impl StreamingToolCallParser {
    /// Create a new streaming parser.
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            in_tool_call_mode: false,
            plain_prefix: String::new(),
        }
    }

    /// Feed a token into the parser and get the resulting event.
    ///
    /// The caller should handle each `StreamEvent` appropriately:
    /// - `TextToken`: forward to the user as streaming text
    /// - `ToolCall`: a complete tool call was parsed
    /// - `Buffering`: the parser is accumulating a potential sentinel; hold output
    /// - `Finished`: end-of-stream finalization
    pub fn feed(&mut self, token: &str) -> StreamEvent {
        self.buffer.push_str(token);

        // If we're already in tool-call mode, just keep buffering.
        // Tool calls will be extracted on finish().
        if self.in_tool_call_mode {
            return StreamEvent::Buffering;
        }

        // Check if the buffer contains the complete sentinel
        if self.buffer.contains(TOOL_CALLS_SENTINEL) {
            self.in_tool_call_mode = true;
            // Extract any plain text before the sentinel
            if let Some(idx) = self.buffer.find(TOOL_CALLS_SENTINEL) {
                let prefix = self.buffer[..idx].to_string();
                if !prefix.is_empty() {
                    self.plain_prefix = prefix;
                }
            }
            return StreamEvent::Buffering;
        }

        // Check if the buffer ends with a partial sentinel prefix.
        // For example, if we've received "[TOOL" we should buffer rather than
        // emit those characters as text.
        if has_partial_sentinel_suffix(&self.buffer) {
            return StreamEvent::Buffering;
        }

        // No sentinel detected; emit everything in the buffer as text
        let text = self.buffer.clone();
        self.buffer.clear();
        StreamEvent::TextToken(text)
    }

    /// Signal end of stream and extract any remaining tool calls.
    pub fn finish(self) -> ParseResult {
        if !self.in_tool_call_mode {
            // No tool calls detected in the entire stream
            return ParseResult::PlainText(self.buffer);
        }

        // Parse the accumulated buffer for tool calls
        parse_tool_calls(&self.buffer)
    }
}

impl Default for StreamingToolCallParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Check if the string ends with a prefix of any sentinel marker.
///
/// This detects cases where a sentinel is split across token boundaries.
/// For example, if the buffer ends with "[TOOL" or "[TOOL_CA", we should
/// buffer rather than emit those characters as plain text.
fn has_partial_sentinel_suffix(text: &str) -> bool {
    // Check against both sentinels
    for sentinel in &[TOOL_CALLS_SENTINEL, ARGS_SENTINEL] {
        for prefix_len in 1..sentinel.len() {
            let sentinel_prefix = &sentinel[..prefix_len];
            if text.ends_with(sentinel_prefix) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // Complete-text parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_plain_text_no_tool_calls() {
        let result = parse_tool_calls("Hello, world! This is a plain response.");
        match result {
            ParseResult::PlainText(text) => {
                assert_eq!(text, "Hello, world! This is a plain response.");
            }
            other => panic!("Expected PlainText, got {:?}", other),
        }
    }

    #[test]
    fn test_single_tool_call() {
        let input = r#"[TOOL_CALLS]search_nodes[ARGS]{"query":"test"}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_nodes");
                assert_eq!(calls[0].args, json!({"query": "test"}));
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_multiple_tool_calls() {
        let input = r#"[TOOL_CALLS]search_nodes[ARGS]{"query": "embeddings"}[TOOL_CALLS]search_nodes[ARGS]{"query": "vector search"}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].name, "search_nodes");
                assert_eq!(calls[0].args, json!({"query": "embeddings"}));
                assert_eq!(calls[1].name, "search_nodes");
                assert_eq!(calls[1].args, json!({"query": "vector search"}));
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_call_with_complex_args() {
        let input = r#"[TOOL_CALLS]create_node[ARGS]{"type":"task","title":"Buy groceries","priority":1,"tags":["food","errands"]}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "create_node");
                assert_eq!(
                    calls[0].args,
                    json!({
                        "type": "task",
                        "title": "Buy groceries",
                        "priority": 1,
                        "tags": ["food", "errands"]
                    })
                );
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_call_with_prefix_text() {
        let input = r#"Let me search for that. [TOOL_CALLS]search_nodes[ARGS]{"query":"test"}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_nodes");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_malformed_json() {
        let input = r#"[TOOL_CALLS]search_nodes[ARGS]{not valid json}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::Error(msg) => {
                assert!(msg.contains("Invalid JSON"), "Error was: {}", msg);
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_missing_args_sentinel() {
        let input = "[TOOL_CALLS]search_nodes{\"query\":\"test\"}";
        let result = parse_tool_calls(input);
        match result {
            ParseResult::Error(msg) => {
                assert!(msg.contains("Missing [ARGS]"), "Error was: {}", msg);
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_empty_function_name() {
        let input = r#"[TOOL_CALLS][ARGS]{"query":"test"}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::Error(msg) => {
                assert!(msg.contains("Empty function name"), "Error was: {}", msg);
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_empty_args() {
        let input = "[TOOL_CALLS]search_nodes[ARGS]";
        let result = parse_tool_calls(input);
        match result {
            ParseResult::Error(msg) => {
                assert!(msg.contains("Empty arguments"), "Error was: {}", msg);
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_empty_string() {
        let result = parse_tool_calls("");
        match result {
            ParseResult::PlainText(text) => {
                assert_eq!(text, "");
            }
            other => panic!("Expected PlainText, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_call_with_nested_json() {
        let input = r#"[TOOL_CALLS]update_node[ARGS]{"id":"node:123","changes":{"title":"Updated","metadata":{"priority":1,"nested":{"deep":true}}}}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "update_node");
                assert_eq!(
                    calls[0].args["changes"]["metadata"]["nested"]["deep"],
                    json!(true)
                );
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_call_with_whitespace() {
        let input = r#"[TOOL_CALLS] search_nodes [ARGS] {"query": "test"} "#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_nodes");
                assert_eq!(calls[0].args, json!({"query": "test"}));
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_three_tool_calls() {
        let input =
            r#"[TOOL_CALLS]a[ARGS]{"x":1}[TOOL_CALLS]b[ARGS]{"y":2}[TOOL_CALLS]c[ARGS]{"z":3}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 3);
                assert_eq!(calls[0].name, "a");
                assert_eq!(calls[1].name, "b");
                assert_eq!(calls[2].name, "c");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Partial sentinel detection tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_partial_sentinel_suffix() {
        assert!(has_partial_sentinel_suffix("some text["));
        assert!(has_partial_sentinel_suffix("some text[T"));
        assert!(has_partial_sentinel_suffix("some text[TOOL"));
        assert!(has_partial_sentinel_suffix("some text[TOOL_"));
        assert!(has_partial_sentinel_suffix("some text[TOOL_CALLS"));
        // The full sentinel "[TOOL_CALLS]" is not a *partial* prefix — it's the
        // complete sentinel, handled by `contains(TOOL_CALLS_SENTINEL)` earlier
        // in the streaming parser.
        assert!(!has_partial_sentinel_suffix("some text[TOOL_CALLS]"));
        assert!(!has_partial_sentinel_suffix("some text"));
        assert!(!has_partial_sentinel_suffix("some text with brackets []"));
    }

    #[test]
    fn test_partial_args_sentinel_suffix() {
        assert!(has_partial_sentinel_suffix("fn_name[A"));
        assert!(has_partial_sentinel_suffix("fn_name[AR"));
        assert!(has_partial_sentinel_suffix("fn_name[ARG"));
        assert!(has_partial_sentinel_suffix("fn_name[ARGS"));
    }

    // -----------------------------------------------------------------------
    // Streaming parser tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_streaming_plain_text() {
        let mut parser = StreamingToolCallParser::new();

        let event1 = parser.feed("Hello");
        assert_eq!(event1, StreamEvent::TextToken("Hello".to_string()));

        let event2 = parser.feed(", world!");
        assert_eq!(event2, StreamEvent::TextToken(", world!".to_string()));

        let result = parser.finish();
        match result {
            ParseResult::PlainText(text) => {
                // After all tokens were emitted, remaining buffer is empty
                assert!(text.is_empty());
            }
            other => panic!("Expected PlainText, got {:?}", other),
        }
    }

    #[test]
    fn test_streaming_tool_call_single_token() {
        // Entire tool call arrives in one token
        let mut parser = StreamingToolCallParser::new();

        let event = parser.feed(r#"[TOOL_CALLS]search_nodes[ARGS]{"query":"test"}"#);
        assert_eq!(event, StreamEvent::Buffering);

        let result = parser.finish();
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_nodes");
                assert_eq!(calls[0].args, json!({"query": "test"}));
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_streaming_tool_call_split_sentinel() {
        // The [TOOL_CALLS] sentinel is split across multiple tokens
        let mut parser = StreamingToolCallParser::new();

        let e1 = parser.feed("[TOOL");
        assert_eq!(e1, StreamEvent::Buffering, "Should buffer partial sentinel");

        let e2 = parser.feed("_CALLS]");
        assert_eq!(
            e2,
            StreamEvent::Buffering,
            "Should still buffer after sentinel completes"
        );

        let e3 = parser.feed("search_nodes");
        assert_eq!(e3, StreamEvent::Buffering);

        let e4 = parser.feed("[ARGS]");
        assert_eq!(e4, StreamEvent::Buffering);

        let e5 = parser.feed(r#"{"query":"test"}"#);
        assert_eq!(e5, StreamEvent::Buffering);

        let result = parser.finish();
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_nodes");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_streaming_text_then_tool_call() {
        let mut parser = StreamingToolCallParser::new();

        // Plain text first
        let e1 = parser.feed("Let me search");
        assert_eq!(e1, StreamEvent::TextToken("Let me search".to_string()));

        let e2 = parser.feed(" for that.");
        assert_eq!(e2, StreamEvent::TextToken(" for that.".to_string()));

        // Then tool call
        let e3 = parser.feed("[TOOL_CALLS]search_nodes[ARGS]");
        assert_eq!(e3, StreamEvent::Buffering);

        let e4 = parser.feed(r#"{"query":"test"}"#);
        assert_eq!(e4, StreamEvent::Buffering);

        let result = parser.finish();
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_nodes");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_streaming_partial_bracket_then_plain() {
        // Edge case: a "[" appears but it's not a sentinel
        let mut parser = StreamingToolCallParser::new();

        let e1 = parser.feed("array[0]");
        // The "[" at position 5 is a potential partial sentinel suffix for "[A"
        // but "array[0]" doesn't end with a sentinel prefix, so it should emit
        assert_eq!(e1, StreamEvent::TextToken("array[0]".to_string()));
    }

    #[test]
    fn test_streaming_partial_bracket_at_end() {
        let mut parser = StreamingToolCallParser::new();

        let e1 = parser.feed("some text[");
        assert_eq!(e1, StreamEvent::Buffering, "Trailing [ should buffer");

        // Next token completes a non-sentinel pattern
        let e2 = parser.feed("0]");
        // Now buffer is "some text[0]" which doesn't end with a sentinel prefix
        assert_eq!(e2, StreamEvent::TextToken("some text[0]".to_string()));
    }

    #[test]
    fn test_streaming_multiple_tool_calls() {
        let mut parser = StreamingToolCallParser::new();

        let input = r#"[TOOL_CALLS]a[ARGS]{"x":1}[TOOL_CALLS]b[ARGS]{"y":2}"#;
        let e1 = parser.feed(input);
        assert_eq!(e1, StreamEvent::Buffering);

        let result = parser.finish();
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 2);
                assert_eq!(calls[0].name, "a");
                assert_eq!(calls[1].name, "b");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_streaming_empty_stream() {
        let parser = StreamingToolCallParser::new();
        let result = parser.finish();
        match result {
            ParseResult::PlainText(text) => assert!(text.is_empty()),
            other => panic!("Expected PlainText, got {:?}", other),
        }
    }

    #[test]
    fn test_streaming_character_by_character() {
        // Feed the tool call character by character to stress-test boundary detection
        let mut parser = StreamingToolCallParser::new();
        let input = r#"[TOOL_CALLS]fn[ARGS]{"k":"v"}"#;

        let mut last_event = StreamEvent::Buffering;
        for ch in input.chars() {
            last_event = parser.feed(&ch.to_string());
        }

        // After feeding all characters, we should be in buffering mode
        assert_eq!(last_event, StreamEvent::Buffering);

        let result = parser.finish();
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "fn");
                assert_eq!(calls[0].args, json!({"k": "v"}));
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_call_with_string_containing_brackets() {
        // JSON args contain bracket characters that look like sentinels
        let input = r#"[TOOL_CALLS]search_nodes[ARGS]{"query":"array[0] and list[1]"}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].args["query"], "array[0] and list[1]");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_tool_call_with_empty_object_args() {
        let input = r#"[TOOL_CALLS]list_tools[ARGS]{}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "list_tools");
                assert_eq!(calls[0].args, json!({}));
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }
}
