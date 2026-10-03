use regex::Regex;
use std::sync::OnceLock;

fn markdown_uri_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[([^\]]+)\]\((nodespace://[^)]+)\)").unwrap())
}

fn backtick_uri_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`(nodespace://[^`]+)`").unwrap())
}

/// Matches a `nodespace://` URI that holds a backslash, up to the whitespace
/// or bracket that ends it.
fn escaped_uri_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"nodespace://[^\s)\]]*\\[^\s)\]]*").unwrap())
}

/// Matches Gemma's textual tool-call / tool-response syntax leaking into prose.
///
/// When the tool-less final inference still tries to "speak" tool activity, the
/// model writes it as literal text like
/// `call:update_schema{...}response:update_schema{...}` instead of emitting a
/// real tool-call token. These segments are internal plumbing, never something
/// the user should see, so we delete each `call:`/`response:` keyword plus its
/// trailing `{...}` argument/result blob.
fn tool_call_prose_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // `(?s)` so the brace blob may span newlines. The blob is a JSON object that
    // may nest (e.g. `{value:{success:true}}` or `{rels:[{name:x}]}`); the
    // `rust regex` crate has no recursion, so we hand-unroll the nesting: an
    // outer `{…}` whose body is any mix of non-brace text and fully-paired inner
    // groups that themselves allow one more level — i.e. up to TWO nested levels
    // (three braces deep total), which spans the shapes these leaks actually
    // take. A blob deeper than that, or one truncated by the token cap
    // (unbalanced braces), simply fails to match and is left intact — safer than
    // greedily eating real trailing prose. Being a DFA, the engine has no
    // catastrophic-backtracking (ReDoS) risk on pathological input.
    RE.get_or_init(|| {
        Regex::new(r"(?s)(?:call|response):[a-z_]+\s*\{(?:[^{}]|\{(?:[^{}]|\{[^{}]*\})*\})*\}")
            .unwrap()
    })
}

/// Matches Gemma's "channel" reasoning blocks leaking into prose.
///
/// Gemma 4 sometimes emits an internal chain-of-thought wrapped in channel
/// markers, e.g. `<|channel>thought\n…reasoning…<channel|>actual answer`. The
/// thought between the markers is internal and must never reach the user. We
/// strip the opening marker plus everything up to and including the closing
/// `<channel|>` marker, leaving the real answer that follows it.
fn channel_block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<\|channel>.*?<channel\|>").unwrap())
}

/// Matches any stray Gemma channel marker not consumed as a full block.
///
/// Covers a dangling `<|channel>` with no closing marker (truncated by the
/// token cap) or a `<channel|>` with no opener — either way the marker itself
/// is plumbing and is removed.
fn stray_channel_marker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<\|channel>|<channel\|>").unwrap())
}

/// Normalize LLM response text for consistent formatting.
///
/// Delegates to [`normalize_response_traced`] and discards the stripper list.
pub fn normalize_response(text: &str) -> String {
    normalize_response_traced(text).0
}

/// Normalize LLM response text and return which strippers modified it.
///
/// `normalize_response` delegates here, making this the single pipeline
/// definition — adding or reordering a stripper only needs one change.
/// The `strippers_fired` list is used by the OTLP tracing path to populate
/// the `response_processing` span's `strippers_fired` attribute.
pub fn normalize_response_traced(text: &str) -> (String, Vec<&'static str>) {
    if text.is_empty() {
        return (String::new(), Vec::new());
    }

    let mut fired: Vec<&'static str> = Vec::new();

    let apply = |name: &'static str,
                 f: fn(&str) -> String,
                 input: String,
                 fired: &mut Vec<&'static str>|
     -> String {
        let out = f(&input);
        if out != input {
            fired.push(name);
        }
        out
    };

    let result = apply(
        "strip_channel_blocks",
        strip_channel_blocks,
        text.to_owned(),
        &mut fired,
    );
    let result = apply(
        "strip_tool_call_prose",
        strip_tool_call_prose,
        result,
        &mut fired,
    );
    let result = apply("unescape_uri_ids", unescape_uri_ids, result, &mut fired);
    let result = apply(
        "collapse_uri_labelled_links",
        collapse_uri_labelled_links,
        result,
        &mut fired,
    );
    let result = apply(
        "fix_backtick_wrapped_uris",
        fix_backtick_wrapped_uris,
        result,
        &mut fired,
    );
    let result = apply(
        "normalize_snake_case_statuses",
        normalize_snake_case_statuses,
        result,
        &mut fired,
    );
    let result = apply(
        "strip_raw_tool_output_json",
        strip_raw_tool_output_json,
        result,
        &mut fired,
    );
    let result = apply(
        "collapse_blank_lines",
        collapse_blank_lines,
        result,
        &mut fired,
    );
    (result.trim().to_string(), fired)
}

/// Remove Markdown escapes from the id of a `nodespace://` URI.
///
/// `nodespace://design\_decision` -> `nodespace://design_decision`
///
/// Writing a list, the model escapes the underscores of an id as it would in
/// prose. The escaped form is not the id any tool returned: the link does not
/// open, and the reply reads as naming a node that does not exist, so it is
/// replaced whole. An id is alphanumeric plus `-` and `_`, so a backslash
/// before either is never part of one. Any other backslash stays as written.
fn unescape_uri_ids(text: &str) -> String {
    escaped_uri_re()
        .replace_all(text, |caps: &regex::Captures| {
            caps[0].replace("\\_", "_").replace("\\-", "-")
        })
        .into_owned()
}

/// Collapse a node link whose label is its own URI to the bare URI.
///
/// `[nodespace://abc-123](nodespace://abc-123)` -> `nodespace://abc-123`
///
/// A titled link, `[Node Title](nodespace://abc-123)`, is the reference form
/// the agent is told to write and passes through untouched: the chat renderer
/// shows it as a link to the node. A URI label carries no title, and the renderer wraps
/// every bare URI in a link of its own, so one left inside a label would nest.
fn collapse_uri_labelled_links(text: &str) -> String {
    let re = markdown_uri_re();
    re.replace_all(text, |caps: &regex::Captures| {
        let link_text = &caps[1];
        let uri = &caps[2];
        if link_text == uri {
            uri.to_string()
        } else {
            caps[0].to_string()
        }
    })
    .into_owned()
}

/// Fix nodespace:// URIs wrapped in backticks.
///
/// `` `nodespace://abc-123` `` -> `nodespace://abc-123`
fn fix_backtick_wrapped_uris(text: &str) -> String {
    let re = backtick_uri_re();
    re.replace_all(text, "$1").into_owned()
}

/// Normalize snake_case status values to Title Case.
///
/// Only applies outside of code blocks and URIs.
fn normalize_snake_case_statuses(text: &str) -> String {
    // Known snake_case status patterns
    let statuses: &[(&str, &str)] = &[
        ("in_progress", "In Progress"),
        ("not_started", "Not Started"),
        ("code_review", "Code Review"),
        ("on_hold", "On Hold"),
        ("in_review", "In Review"),
        ("to_do", "To Do"),
    ];

    let mut result = String::new();
    let mut in_code_fence = false;

    for line in text.split('\n') {
        if !result.is_empty() {
            result.push('\n');
        }

        // Track fenced code blocks
        if line.trim_start().starts_with("```") {
            in_code_fence = !in_code_fence;
            result.push_str(line);
            continue;
        }

        if in_code_fence {
            result.push_str(line);
            continue;
        }

        // Process the line character by character to skip inline code and URIs
        let mut processed_line = line.to_string();
        for &(pattern, replacement) in statuses {
            // Build a regex that matches the pattern but NOT inside backticks or URIs
            // We do a simple approach: split by backtick segments and nodespace:// URIs
            processed_line = replace_status_outside_special(&processed_line, pattern, replacement);
        }
        result.push_str(&processed_line);
    }

    result
}

/// Replace a status pattern in text, skipping inline code spans and URIs.
fn replace_status_outside_special(line: &str, pattern: &str, replacement: &str) -> String {
    let mut result = String::new();
    let mut remaining = line;

    while !remaining.is_empty() {
        // Find the next backtick or nodespace:// URI
        let next_backtick = remaining.find('`');
        let next_uri = remaining.find("nodespace://");

        let skip_start = match (next_backtick, next_uri) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };

        match skip_start {
            Some(pos) => {
                // Process text before the special region
                let before = &remaining[..pos];
                result.push_str(&before.replace(pattern, replacement));

                remaining = &remaining[pos..];

                if remaining.starts_with('`') {
                    // Skip inline code span
                    if let Some(end) = remaining[1..].find('`') {
                        let code_span = &remaining[..end + 2];
                        result.push_str(code_span);
                        remaining = &remaining[end + 2..];
                    } else {
                        // Unmatched backtick, just push rest
                        result.push_str(remaining);
                        return result;
                    }
                } else if remaining.starts_with("nodespace://") {
                    // Skip until whitespace or end
                    let end = remaining
                        .find(|c: char| c.is_whitespace() || c == ')' || c == ']')
                        .unwrap_or(remaining.len());
                    result.push_str(&remaining[..end]);
                    remaining = &remaining[end..];
                }
            }
            None => {
                // No more special regions, process the rest
                result.push_str(&remaining.replace(pattern, replacement));
                break;
            }
        }
    }

    result
}

/// Strip leaked Gemma channel/thought reasoning blocks.
///
/// First removes complete `<|channel>…<channel|>` spans. If a dangling opener
/// remains (closing marker lost to the token cap), everything from that opener
/// onward is internal thought and is dropped. Any stray markers left over are
/// then removed. See [`channel_block_re`] / [`stray_channel_marker_re`].
fn strip_channel_blocks(text: &str) -> String {
    if !text.contains("<|channel>") && !text.contains("<channel|>") {
        return text.to_string();
    }
    let result = channel_block_re().replace_all(text, "").into_owned();
    // Drop a dangling, never-closed thought block (truncated mid-reasoning).
    let result = match result.find("<|channel>") {
        Some(idx) => result[..idx].to_string(),
        None => result,
    };
    stray_channel_marker_re()
        .replace_all(&result, "")
        .into_owned()
}

/// Strip leaked Gemma tool-call/response prose (`call:foo{...}response:foo{...}`).
///
/// See [`tool_call_prose_re`]. If stripping these segments leaves nothing but
/// whitespace, the whole response was tool-call plumbing — return empty so the
/// caller's "empty response" handling (a synthesized confirmation) kicks in
/// rather than surfacing a blank bubble.
fn strip_tool_call_prose(text: &str) -> String {
    if !text.contains("call:") && !text.contains("response:") {
        return text.to_string();
    }
    tool_call_prose_re().replace_all(text, "").into_owned()
}

/// Strip raw JSON blocks that look like pasted tool output.
///
/// Detects blocks like `{"count": 3, "nodes": [...]}` that appear outside
/// of code fences. Only strips JSON containing tool-output-like keys.
fn strip_raw_tool_output_json(text: &str) -> String {
    let tool_output_keys = ["\"count\"", "\"nodes\"", "\"node_type\"", "\"id\""];

    let mut result = String::new();
    let mut in_code_fence = false;
    let mut json_block = String::new();
    let mut brace_depth: i32 = 0;
    let mut in_json = false;

    for line in text.split('\n') {
        if !result.is_empty() && !in_json {
            result.push('\n');
        }

        // Track fenced code blocks
        if line.trim_start().starts_with("```") {
            in_code_fence = !in_code_fence;
            result.push_str(line);
            continue;
        }

        if in_code_fence {
            result.push_str(line);
            continue;
        }

        // Detect start of a JSON block (line starting with `{`)
        if !in_json && line.trim_start().starts_with('{') {
            in_json = true;
            json_block.clear();
        }

        if in_json {
            if !json_block.is_empty() {
                json_block.push('\n');
            }
            json_block.push_str(line);
            brace_depth += line.chars().filter(|&c| c == '{').count() as i32;
            brace_depth -= line.chars().filter(|&c| c == '}').count() as i32;

            if brace_depth <= 0 {
                in_json = false;
                brace_depth = 0;

                // Check if it looks like tool output
                let looks_like_tool_output =
                    tool_output_keys.iter().any(|key| json_block.contains(key));

                // Validate it's actual JSON-ish (starts with { and ends with })
                let trimmed = json_block.trim();
                let is_json_shaped = trimmed.starts_with('{') && trimmed.ends_with('}');

                if !(looks_like_tool_output && is_json_shaped) {
                    // Keep it - not tool output
                    result.push_str(&json_block);
                }
                json_block.clear();
            }
            continue;
        }

        result.push_str(line);
    }

    // If we ended mid-JSON, keep it (malformed, don't strip)
    if !json_block.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(&json_block);
    }

    result
}

/// Collapse multiple consecutive blank lines into a single blank line,
/// and trim trailing whitespace from each line.
fn collapse_blank_lines(text: &str) -> String {
    let mut result = String::new();
    let mut prev_blank = false;

    for line in text.split('\n') {
        let trimmed_end = line.trim_end();
        if trimmed_end.is_empty() {
            if prev_blank {
                continue;
            }
            prev_blank = true;
            result.push('\n');
        } else {
            prev_blank = false;
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(trimmed_end);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // URI normalization

    #[test]
    fn fixes_markdown_link_with_uri_as_text() {
        let input = "Check [nodespace://abc-123](nodespace://abc-123) for details.";
        let result = normalize_response(input);
        assert_eq!(result, "Check nodespace://abc-123 for details.");
    }

    #[test]
    fn leaves_titled_markdown_link_unchanged() {
        let input = "See [My Task](nodespace://abc-123) for more info.";
        let (result, fired) = normalize_response_traced(input);
        assert_eq!(result, input);
        assert!(fired.is_empty(), "no strippers should fire: {:?}", fired);
    }

    #[test]
    fn fixes_backtick_wrapped_uri() {
        let input = "Open `nodespace://abc-123` to view.";
        let result = normalize_response(input);
        assert_eq!(result, "Open nodespace://abc-123 to view.");
    }

    #[test]
    fn leaves_bare_uri_unchanged() {
        let input = "Open nodespace://abc-123 to view.";
        let result = normalize_response(input);
        assert_eq!(result, "Open nodespace://abc-123 to view.");
    }

    #[test]
    fn handles_multiple_uris_in_one_response() {
        let input =
            "See [nodespace://a](nodespace://a) and `nodespace://b` and [Title](nodespace://c).";
        let result = normalize_response(input);
        assert_eq!(
            result,
            "See nodespace://a and nodespace://b and [Title](nodespace://c)."
        );
    }

    #[test]
    fn unescapes_underscores_and_hyphens_in_a_uri_id() {
        let input =
            r"See [Design Decision](nodespace://design\_decision) and nodespace://a1b2\-c3d4.";
        let (result, fired) = normalize_response_traced(input);
        assert_eq!(
            result,
            "See [Design Decision](nodespace://design_decision) and nodespace://a1b2-c3d4."
        );
        assert_eq!(fired, vec!["unescape_uri_ids"]);
    }

    #[test]
    fn leaves_escapes_outside_a_uri_unchanged() {
        // An escaped underscore in prose is the model's own formatting, and a
        // link's label is prose.
        let input = r"The field due\_date on [my\_type](nodespace://my_type) is set.";
        let (result, fired) = normalize_response_traced(input);
        assert_eq!(result, input);
        assert!(fired.is_empty(), "no strippers should fire: {:?}", fired);
    }

    #[test]
    fn leaves_other_backslashes_in_a_uri_unchanged() {
        // Only the two characters an id can hold are unescaped. Anything else
        // stays as written, for the fabricated-id check to judge.
        let input = r"See nodespace://abc\*def for details.";
        assert_eq!(normalize_response(input), input);
    }

    // Status normalization

    #[test]
    fn normalizes_snake_case_status() {
        let input = "The task is in_progress and another is not_started.";
        let result = normalize_response(input);
        assert_eq!(
            result,
            "The task is In Progress and another is Not Started."
        );
    }

    #[test]
    fn does_not_normalize_inside_code_blocks() {
        let input = "```\nstatus = in_progress\n```";
        let result = normalize_response(input);
        assert_eq!(result, "```\nstatus = in_progress\n```");
    }

    #[test]
    fn does_not_normalize_inside_inline_code() {
        let input = "Use `in_progress` as the value.";
        let result = normalize_response(input);
        assert_eq!(result, "Use `in_progress` as the value.");
    }

    #[test]
    fn does_not_normalize_inside_uris() {
        // URIs shouldn't contain these patterns normally, but verify safety
        let input = "Link: nodespace://task_in_progress_123";
        let result = normalize_response(input);
        // The URI should not be mangled
        assert!(result.contains("nodespace://"));
    }

    // JSON stripping

    #[test]
    fn strips_raw_tool_output_json() {
        let input =
            "Here are the results:\n{\"count\": 3, \"nodes\": [\"a\", \"b\", \"c\"]}\nThat's all.";
        let result = normalize_response(input);
        assert_eq!(result, "Here are the results:\n\nThat's all.");
    }

    #[test]
    fn preserves_json_inside_code_fences() {
        let input = "Example:\n```json\n{\"count\": 3, \"nodes\": []}\n```\nDone.";
        let result = normalize_response(input);
        assert!(result.contains("\"count\": 3"));
        assert!(result.contains("```json"));
    }

    #[test]
    fn preserves_non_tool_json() {
        let input = "Config:\n{\"theme\": \"dark\", \"lang\": \"en\"}\nApplied.";
        let result = normalize_response(input);
        assert!(result.contains("\"theme\": \"dark\""));
    }

    // Whitespace

    #[test]
    fn collapses_multiple_blank_lines() {
        let input = "Hello\n\n\n\nWorld";
        let result = normalize_response(input);
        assert_eq!(result, "Hello\n\nWorld");
    }

    #[test]
    fn trims_response() {
        let input = "  \n  Hello World  \n  ";
        let result = normalize_response(input);
        assert_eq!(result, "Hello World");
    }

    // Edge cases

    #[test]
    fn empty_input_returns_empty() {
        assert_eq!(normalize_response(""), "");
    }

    #[test]
    fn clean_input_passes_through_unchanged() {
        let input = "This is a clean response with no issues.";
        let result = normalize_response(input);
        assert_eq!(result, input);
    }

    #[test]
    fn complex_mixed_content() {
        let input = concat!(
            "Here is your task [My Task](nodespace://task-001).\n",
            "\n",
            "Status: in_progress\n",
            "\n",
            "\n",
            "\n",
            "{\"count\": 1, \"nodes\": [{\"id\": \"task-001\"}]}\n",
            "\n",
            "Also see `nodespace://note-002` for context.\n",
            "\n",
            "```\ncode_review status\n```\n",
            "\n",
            "That's everything.  "
        );
        let result = normalize_response(input);
        assert!(result.starts_with("Here is your task [My Task](nodespace://task-001)."));
        assert!(result.contains("Status: In Progress"));
        assert!(!result.contains("\"count\""));
        assert!(result.contains("nodespace://note-002"));
        assert!(result.contains("code_review")); // inside code fence
        assert!(result.ends_with("That's everything."));
        // No triple+ blank lines
        assert!(!result.contains("\n\n\n"));
    }

    #[test]
    fn strips_multiline_tool_output_json() {
        let input = "Results:\n{\n  \"count\": 5,\n  \"nodes\": [\n    \"a\"\n  ]\n}\nDone.";
        let result = normalize_response(input);
        assert_eq!(result, "Results:\n\nDone.");
    }

    #[test]
    fn strips_leaked_tool_call_prose() {
        // Gemma's textual tool-call/response syntax must never reach the user.
        let input = "call:update_schema{add_relationships:[{name:invoices_for}],schema_id:product}response:update_schema{value:{success:true}}";
        let result = normalize_response(input);
        assert_eq!(result, "", "leaked tool-call prose should strip to empty");
    }

    #[test]
    fn strips_tool_call_prose_keeping_surrounding_text() {
        let input = "Created the schema. call:create_schema{name:Invoice} All set.";
        let result = normalize_response(input);
        assert_eq!(result, "Created the schema.  All set.");
    }

    #[test]
    fn leaves_prose_mentioning_call_unchanged() {
        // "call:" without a following brace blob is ordinary prose, not a leak.
        let input = "Give me a call: I'll explain the response: it's ready.";
        let result = normalize_response(input);
        assert_eq!(result, input);
    }

    #[test]
    fn strips_channel_thought_block_keeping_answer() {
        let input =
            "Done.<|channel>thought\nThe user wants X, I should Y.<channel|>I've added the node!";
        let result = normalize_response(input);
        assert_eq!(result, "Done.I've added the node!");
    }

    #[test]
    fn strips_dangling_channel_block_truncated_by_token_cap() {
        // No closing marker (generation hit the cap mid-thought): drop the rest.
        let input = "Here is your answer.<|channel>thought\nNow I will reason endlessly";
        let result = normalize_response(input);
        assert_eq!(result, "Here is your answer.");
    }

    #[test]
    fn strips_stray_channel_marker() {
        let input = "All set.<channel|>";
        let result = normalize_response(input);
        assert_eq!(result, "All set.");
    }

    // normalize_response_traced: strippers_fired correctness

    #[test]
    fn traced_no_strippers_fire_on_clean_text() {
        let (normalized, fired) = normalize_response_traced("Hello! How can I help?");
        assert_eq!(normalized, "Hello! How can I help?");
        assert!(fired.is_empty(), "no strippers should fire: {:?}", fired);
    }

    #[test]
    fn traced_strip_tool_call_prose_fires() {
        let input = "Here is my answer call:update_node{id:\"abc\",value:\"x\"} done.";
        let (_, fired) = normalize_response_traced(input);
        assert!(
            fired.contains(&"strip_tool_call_prose"),
            "expected strip_tool_call_prose in {:?}",
            fired
        );
    }

    #[test]
    fn traced_strip_channel_blocks_fires() {
        let input = "<|channel>internal thought<channel|>Actual reply.";
        let (normalized, fired) = normalize_response_traced(input);
        assert_eq!(normalized, "Actual reply.");
        assert!(
            fired.contains(&"strip_channel_blocks"),
            "expected strip_channel_blocks in {:?}",
            fired
        );
    }

    #[test]
    fn normalize_response_delegates_to_traced() {
        // Verify the two entry-points produce identical output so the pipeline
        // definition can never silently diverge.
        let inputs = [
            "Hello!",
            "<|channel>thought<channel|>Answer.",
            "call:foo{bar:baz} done.",
            "in_progress",
            "  \n\n  multiple\n\n\n\nblank lines\n\n  ",
        ];
        for input in &inputs {
            assert_eq!(
                normalize_response(input),
                normalize_response_traced(input).0,
                "mismatch for input: {input:?}"
            );
        }
    }
}
