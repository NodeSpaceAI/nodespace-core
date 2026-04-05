use regex::Regex;

/// Normalize LLM response text for consistent formatting.
///
/// Applies post-processing rules to fix common formatting issues
/// from small local models that don't always follow system prompt instructions.
pub fn normalize_response(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    let result = fix_markdown_link_uris(text);
    let result = fix_backtick_wrapped_uris(&result);
    let result = normalize_snake_case_statuses(&result);
    let result = strip_raw_tool_output_json(&result);
    let result = collapse_blank_lines(&result);
    result.trim().to_string()
}

/// Fix nodespace:// URIs wrapped in markdown links.
///
/// - `[nodespace://abc-123](nodespace://abc-123)` -> `nodespace://abc-123`
/// - `[Node Title](nodespace://abc-123)` -> `**Node Title** (nodespace://abc-123)`
fn fix_markdown_link_uris(text: &str) -> String {
    let re = Regex::new(r"\[([^\]]+)\]\((nodespace://[^)]+)\)").unwrap();
    re.replace_all(text, |caps: &regex::Captures| {
        let link_text = &caps[1];
        let uri = &caps[2];
        if link_text == uri {
            // [nodespace://abc](nodespace://abc) -> nodespace://abc
            uri.to_string()
        } else {
            // [Title](nodespace://abc) -> **Title** (nodespace://abc)
            format!("**{link_text}** ({uri})")
        }
    })
    .into_owned()
}

/// Fix nodespace:// URIs wrapped in backticks.
///
/// `` `nodespace://abc-123` `` -> `nodespace://abc-123`
fn fix_backtick_wrapped_uris(text: &str) -> String {
    let re = Regex::new(r"`(nodespace://[^`]+)`").unwrap();
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
    fn fixes_markdown_link_with_title() {
        let input = "See [My Task](nodespace://abc-123) for more info.";
        let result = normalize_response(input);
        assert_eq!(
            result,
            "See **My Task** (nodespace://abc-123) for more info."
        );
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
            "See nodespace://a and nodespace://b and **Title** (nodespace://c)."
        );
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
        assert!(result.starts_with("Here is your task **My Task** (nodespace://task-001)."));
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
}
