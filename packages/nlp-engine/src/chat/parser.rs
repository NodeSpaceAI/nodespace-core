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
/// Supports two Mistral tool call formats:
///
/// **Format A** (legacy, with `[ARGS]` sentinel):
/// ```text
/// [TOOL_CALLS]function_name[ARGS]{"param": "value"}
/// ```
///
/// **Format B** (Ministral 2512+, no `[ARGS]` sentinel):
/// ```text
/// [TOOL_CALLS]function_name{"param": "value"}
/// ```
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

        // Determine format: check if [ARGS] comes before the first `{`
        let has_args_sentinel = after_sentinel.find(ARGS_SENTINEL);
        let has_json_start = after_sentinel.find('{');

        let (function_name, json_str, advance_to) = if let (Some(args_pos), Some(json_pos)) =
            (has_args_sentinel, has_json_start)
        {
            if args_pos < json_pos {
                // Format A: [TOOL_CALLS]name[ARGS]{json}
                let name = after_sentinel[..args_pos].trim().to_string();
                let after_args = &after_sentinel[args_pos + ARGS_SENTINEL.len()..];
                let json_end = after_args
                    .find(TOOL_CALLS_SENTINEL)
                    .unwrap_or(after_args.len());
                let json = after_args[..json_end].trim();
                (name, json, &after_args[json_end..])
            } else {
                // Format B: [TOOL_CALLS]name{json}
                match parse_format_b(after_sentinel) {
                    Ok(v) => v,
                    Err(e) => return e,
                }
            }
        } else if has_json_start.is_some() {
            // No [ARGS] sentinel, but JSON found — Format B
            match parse_format_b(after_sentinel) {
                Ok(v) => v,
                Err(e) => return e,
            }
        } else if has_args_sentinel.is_some() {
            // [ARGS] found but no JSON — malformed
            return ParseResult::Error("Found [ARGS] sentinel but no JSON arguments".to_string());
        } else {
            return ParseResult::Error("No arguments found after [TOOL_CALLS]".to_string());
        };

        if function_name.is_empty() {
            return ParseResult::Error("Empty function name after [TOOL_CALLS]".to_string());
        }

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
                    function_name, e, json_str
                ));
            }
        }

        remaining = advance_to;
    }

    if calls.is_empty() {
        ParseResult::Error("Found [TOOL_CALLS] sentinel but parsed zero tool calls".to_string())
    } else {
        ParseResult::ToolCalls(calls)
    }
}

/// Parse Format B: `function_name{"param": "value"}` (no [ARGS] sentinel).
///
/// The function name is everything before the first `{`.
/// The JSON extends from `{` to the matching `}` (brace-balanced).
///
/// Returns `(function_name, json_str, remaining)` or an error.
/// Returns `Ok((name, json, remaining))` or `Err(ParseResult::Error(...))`.
fn parse_format_b(after_sentinel: &str) -> std::result::Result<(String, &str, &str), ParseResult> {
    let json_start = after_sentinel
        .find('{')
        .ok_or_else(|| ParseResult::Error("No JSON found after function name".to_string()))?;

    let function_name = after_sentinel[..json_start].trim().to_string();

    // Find the end of the JSON object by brace-balancing
    let json_region = &after_sentinel[json_start..];
    let json_end = find_balanced_brace(json_region).ok_or_else(|| {
        ParseResult::Error(format!(
            "Unbalanced braces in tool call arguments for '{}'",
            function_name
        ))
    })?;

    let json_str = json_region[..json_end].trim();
    let after_json = &json_region[json_end..];

    // Check for another [TOOL_CALLS] or end
    let advance_to = if let Some(next_tc) = after_json.find(TOOL_CALLS_SENTINEL) {
        &after_json[next_tc..]
    } else {
        // End of string
        &after_json[after_json.len()..]
    };

    Ok((function_name, json_str, advance_to))
}

/// Find the end of a brace-balanced JSON object starting at `{`.
///
/// Returns the byte offset just past the closing `}`, or `None` if braces
/// are unbalanced. Handles strings (skips `{`/`}` inside quoted strings).
fn find_balanced_brace(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, ch) in s.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' && in_string {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(i + 1); // past the closing }
            }
        }
    }
    None
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
    fn test_no_args_sentinel_parses_as_format_b() {
        // Without [ARGS], this is now valid Format B (Ministral 2512+)
        let input = "[TOOL_CALLS]search_nodes{\"query\":\"test\"}";
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_nodes");
                assert_eq!(calls[0].args["query"], "test");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
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
    fn test_empty_args_with_sentinel() {
        // [ARGS] present but no JSON after it — [ARGS] comes before any `{`
        // so it's treated as Format A, but with empty args
        let input = "[TOOL_CALLS]search_nodes[ARGS]";
        let result = parse_tool_calls(input);
        match result {
            ParseResult::Error(msg) => {
                // Any error about missing/empty args is fine
                assert!(
                    msg.contains("arguments") || msg.contains("JSON"),
                    "Error was: {}",
                    msg
                );
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

    // -----------------------------------------------------------------------
    // Format B tests (Ministral 2512+ — no [ARGS] sentinel)
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_b_real_model_output() {
        // Actual output from Ministral 3B/8B 2512 model
        let input = r#"[TOOL_CALLS]search_nodes{"query": "task", "node_type": "task"}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_nodes");
                assert_eq!(calls[0].args["query"], "task");
                assert_eq!(calls[0].args["node_type"], "task");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_format_b_simple() {
        let input = r#"[TOOL_CALLS]get_node{"node_id": "abc-123"}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "get_node");
                assert_eq!(calls[0].args["node_id"], "abc-123");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_format_b_nested_json() {
        let input = r#"[TOOL_CALLS]create_node{"title": "Test", "properties": {"priority": 1, "tags": ["a", "b"]}}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "create_node");
                assert_eq!(calls[0].args["properties"]["priority"], 1);
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_format_b_with_prefix_text() {
        let input = r#"Let me search for that. [TOOL_CALLS]search_nodes{"query": "test"}"#;
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
    fn test_format_b_empty_args() {
        let input = r#"[TOOL_CALLS]list_all{}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "list_all");
                assert_eq!(calls[0].args, json!({}));
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_format_b_json_with_braces_in_strings() {
        let input = r#"[TOOL_CALLS]search_nodes{"query": "find {important} nodes"}"#;
        let result = parse_tool_calls(input);
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].args["query"], "find {important} nodes");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    #[test]
    fn test_format_b_streaming() {
        // Simulate streaming: [TOOL_CALLS] control token injected, then name + json
        let mut parser = StreamingToolCallParser::new();

        let e1 = parser.feed("[TOOL_CALLS]");
        assert_eq!(e1, StreamEvent::Buffering);

        let e2 = parser.feed("search_nodes");
        assert_eq!(e2, StreamEvent::Buffering);

        let e3 = parser.feed(r#"{"query": "test"}"#);
        assert_eq!(e3, StreamEvent::Buffering);

        let result = parser.finish();
        match result {
            ParseResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_nodes");
                assert_eq!(calls[0].args["query"], "test");
            }
            other => panic!("Expected ToolCalls, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Brace balancing tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_balanced_brace_simple() {
        assert_eq!(find_balanced_brace(r#"{"a": 1}"#), Some(8));
    }

    #[test]
    fn test_balanced_brace_nested() {
        assert_eq!(find_balanced_brace(r#"{"a": {"b": 1}}"#), Some(15));
    }

    #[test]
    fn test_balanced_brace_with_string() {
        assert_eq!(find_balanced_brace(r#"{"a": "}"}"#), Some(10));
    }

    #[test]
    fn test_balanced_brace_unbalanced() {
        assert_eq!(find_balanced_brace(r#"{"a": 1"#), None);
    }
}
