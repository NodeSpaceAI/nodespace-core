//! Markdown stripping utilities for title extraction
//!
//! This module provides functions to strip markdown formatting from content,
//! producing clean plain text suitable for search indexing and display.

use crate::models::SchemaField;
use regex::Regex;
use std::sync::LazyLock;

/// Compiled regex patterns for markdown stripping
///
/// The order of these patterns matters:
/// 1. Images first (to not conflict with links or italic)
/// 2. Links (before italic since links use brackets)
/// 3. Bold (before italic since ** conflicts with *)
/// 4. Other inline styles
/// 5. Line-start patterns (headers, lists, etc.)
static MARKDOWN_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        // Remove images FIRST: ![alt](url) -> alt
        (Regex::new(r"!\[([^\]]*)\]\([^)]+\)").unwrap(), "$1"),
        // Remove markdown links, keeping link text: [text](url) -> text
        (Regex::new(r"\[([^\]]+)\]\([^)]+\)").unwrap(), "$1"),
        // Remove inline code: `code` -> code
        (Regex::new(r"`([^`]+)`").unwrap(), "$1"),
        // Remove bold: **text** or __text__ -> text (process before italic)
        (Regex::new(r"\*\*([^*]+)\*\*").unwrap(), "$1"),
        (Regex::new(r"__([^_]+)__").unwrap(), "$1"),
        // Remove strikethrough: ~~text~~ -> text
        (Regex::new(r"~~([^~]+)~~").unwrap(), "$1"),
        // Remove italic: *text* or _text_ -> text
        // Process after bold to avoid conflicts
        (Regex::new(r"\*([^*]+)\*").unwrap(), "$1"),
        (Regex::new(r"_([^_]+)_").unwrap(), "$1"),
        // Remove headers: # Header -> Header (up to 6 levels)
        (Regex::new(r"^#{1,6}\s+").unwrap(), ""),
        // Remove blockquote markers: > quote -> quote
        (Regex::new(r"^>\s*").unwrap(), ""),
        // Remove ordered list markers: 1. item -> item
        (Regex::new(r"^\d+\.\s+").unwrap(), ""),
        // Remove unordered list markers: - item or * item -> item
        (Regex::new(r"^[-*+]\s+").unwrap(), ""),
        // Remove horizontal rules
        (Regex::new(r"^[-*_]{3,}$").unwrap(), ""),
        // Remove HTML tags
        (Regex::new(r"<[^>]+>").unwrap(), ""),
        // Remove nodespace:// links (internal references)
        (Regex::new(r"nodespace://[^\s)\]]+").unwrap(), ""),
    ]
});

/// Compiled regex for whitespace normalization
static WHITESPACE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// Strip markdown formatting from content to produce plain text
///
/// This function removes common markdown syntax to produce clean text
/// suitable for search indexing in the `title` field.
///
/// # Arguments
///
/// * `content` - The markdown content to strip
///
/// # Returns
///
/// Plain text with markdown formatting removed
///
/// # Examples
///
/// ```
/// use nodespace_core::utils::strip_markdown;
///
/// assert_eq!(strip_markdown("# Hello World"), "Hello World");
/// assert_eq!(strip_markdown("**bold** text"), "bold text");
/// assert_eq!(strip_markdown("[link](http://example.com)"), "link");
/// assert_eq!(strip_markdown("## Project Planning"), "Project Planning");
/// ```
pub fn strip_markdown(content: &str) -> String {
    let mut result = content.to_string();

    // Apply each pattern
    for (pattern, replacement) in MARKDOWN_PATTERNS.iter() {
        // For line-start patterns, process line by line
        if replacement.is_empty() && pattern.as_str().starts_with('^') {
            result = result
                .lines()
                .map(|line| pattern.replace_all(line, *replacement).to_string())
                .collect::<Vec<_>>()
                .join("\n");
        } else {
            result = pattern.replace_all(&result, *replacement).to_string();
        }
    }

    // Clean up multiple whitespace and trim
    result = WHITESPACE_RE.replace_all(&result, " ").to_string();
    result.trim().to_string()
}

/// Interpolate a title template string using node properties.
///
/// Replaces `{field_name}` tokens with the corresponding property value.
/// Non-string values (numbers, booleans) are converted to their string representation.
/// Missing or null fields are replaced with empty strings.
/// The result is trimmed of leading/trailing whitespace.
///
/// # Arguments
///
/// * `template` - The template string with `{field_name}` placeholders
/// * `properties` - The node's properties JSON object
///
/// # Examples
///
/// ```
/// use nodespace_core::utils::interpolate_title_template;
/// use serde_json::json;
///
/// let props = json!({"first_name": "John", "last_name": "Doe"});
/// assert_eq!(
///     interpolate_title_template("{first_name} {last_name}", &props),
///     "John Doe"
/// );
/// ```
pub fn interpolate_title_template(template: &str, properties: &serde_json::Value) -> String {
    let mut result = String::with_capacity(template.len());
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '{' {
            // Find the closing brace
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '}') {
                let field_name: String = chars[i + 1..i + 1 + end].iter().collect();
                // Look up the field value in properties
                let value = properties.get(&field_name);
                match value {
                    Some(serde_json::Value::String(s)) => result.push_str(s),
                    Some(serde_json::Value::Number(n)) => result.push_str(&n.to_string()),
                    Some(serde_json::Value::Bool(b)) => result.push_str(&b.to_string()),
                    Some(serde_json::Value::Null) | None => {} // empty string for missing/null
                    Some(other) => result.push_str(&other.to_string()),
                }
                i += 1 + end + 1; // skip past '}'
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    // Normalize whitespace and trim
    let normalized = WHITESPACE_RE.replace_all(&result, " ");
    normalized.trim().to_string()
}

/// Interpolate a title template string using node properties, resolving enum labels.
///
/// Like [`interpolate_title_template`], but additionally resolves enum field values
/// to their human-readable labels using the provided schema fields.
///
/// For each `{field_name}` token, if the field is of type `"enum"`, the raw stored
/// value (e.g. `"on_hold"`) is replaced with the matching label (e.g. `"On Hold"`).
/// `core_values` are searched before `user_values`. If no matching label is found,
/// the raw value is used as-is.
///
/// # Arguments
///
/// * `template` - The template string with `{field_name}` placeholders
/// * `properties` - The node's properties JSON object
/// * `fields` - Schema field definitions (from `SchemaNode.fields`)
pub fn interpolate_title_template_with_schema(
    template: &str,
    properties: &serde_json::Value,
    fields: &[SchemaField],
) -> String {
    let mut result = String::with_capacity(template.len());
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '{' {
            if let Some(end) = chars[i + 1..].iter().position(|&c| c == '}') {
                let field_name: String = chars[i + 1..i + 1 + end].iter().collect();
                let value = properties.get(&field_name);

                let raw_string: Option<String> = match value {
                    Some(serde_json::Value::String(s)) => Some(s.clone()),
                    Some(serde_json::Value::Number(n)) => Some(n.to_string()),
                    Some(serde_json::Value::Bool(b)) => Some(b.to_string()),
                    Some(serde_json::Value::Null) | None => None,
                    Some(other) => Some(other.to_string()),
                };

                if let Some(raw) = raw_string {
                    let resolved = fields
                        .iter()
                        .find(|f| f.name == field_name && f.field_type == "enum")
                        .and_then(|f| {
                            f.core_values
                                .iter()
                                .flatten()
                                .chain(f.user_values.iter().flatten())
                                .find(|ev| ev.value == raw)
                                .map(|ev| ev.label.clone())
                        })
                        .unwrap_or(raw);

                    result.push_str(&resolved);
                }

                i += 1 + end + 1;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }

    let normalized = WHITESPACE_RE.replace_all(&result, " ");
    normalized.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_headers() {
        assert_eq!(strip_markdown("# Header 1"), "Header 1");
        assert_eq!(strip_markdown("## Header 2"), "Header 2");
        assert_eq!(strip_markdown("###### Header 6"), "Header 6");
    }

    #[test]
    fn test_strip_bold() {
        assert_eq!(strip_markdown("**bold text**"), "bold text");
        assert_eq!(strip_markdown("__also bold__"), "also bold");
        assert_eq!(
            strip_markdown("text with **bold** word"),
            "text with bold word"
        );
    }

    #[test]
    fn test_strip_italic() {
        assert_eq!(strip_markdown("*italic text*"), "italic text");
        assert_eq!(strip_markdown("_also italic_"), "also italic");
    }

    #[test]
    fn test_strip_links() {
        assert_eq!(
            strip_markdown("[link text](http://example.com)"),
            "link text"
        );
        assert_eq!(
            strip_markdown("Check [this link](http://test.com) out"),
            "Check this link out"
        );
    }

    #[test]
    fn test_strip_images() {
        assert_eq!(strip_markdown("![alt text](image.png)"), "alt text");
        assert_eq!(strip_markdown("![](image.png)"), "");
    }

    #[test]
    fn test_strip_inline_code() {
        assert_eq!(strip_markdown("`code`"), "code");
        assert_eq!(
            strip_markdown("use `println!` function"),
            "use println! function"
        );
    }

    #[test]
    fn test_strip_strikethrough() {
        assert_eq!(strip_markdown("~~deleted~~"), "deleted");
    }

    #[test]
    fn test_strip_blockquotes() {
        assert_eq!(strip_markdown("> quoted text"), "quoted text");
    }

    #[test]
    fn test_strip_list_markers() {
        assert_eq!(strip_markdown("- list item"), "list item");
        assert_eq!(strip_markdown("* another item"), "another item");
        assert_eq!(strip_markdown("1. numbered item"), "numbered item");
    }

    #[test]
    fn test_strip_nodespace_links() {
        assert_eq!(
            strip_markdown("[Meeting Notes](nodespace://abc-123)"),
            "Meeting Notes"
        );
        assert_eq!(
            strip_markdown("See nodespace://2025-01-26 for details"),
            "See for details"
        );
    }

    #[test]
    fn test_strip_html_tags() {
        assert_eq!(strip_markdown("<b>bold</b>"), "bold");
        assert_eq!(strip_markdown("text <br/> more"), "text more");
    }

    #[test]
    fn test_combined_formatting() {
        assert_eq!(
            strip_markdown("# **Bold Header** with [link](url)"),
            "Bold Header with link"
        );
    }

    #[test]
    fn test_plain_text_unchanged() {
        assert_eq!(strip_markdown("Plain text"), "Plain text");
        assert_eq!(strip_markdown("No formatting here"), "No formatting here");
    }

    #[test]
    fn test_empty_and_whitespace() {
        assert_eq!(strip_markdown(""), "");
        assert_eq!(strip_markdown("   "), "");
        assert_eq!(strip_markdown("  text  "), "text");
    }

    #[test]
    fn test_multiline_content() {
        let input = "# Header\n\nSome **bold** text\n- List item";
        let expected = "Header Some bold text List item";
        assert_eq!(strip_markdown(input), expected);
    }

    // -------------------------------------------------------------------------
    // interpolate_title_template tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_interpolate_basic_fields() {
        let props = serde_json::json!({"first_name": "John", "last_name": "Doe"});
        assert_eq!(
            interpolate_title_template("{first_name} {last_name}", &props),
            "John Doe"
        );
    }

    #[test]
    fn test_interpolate_missing_field_is_empty() {
        let props = serde_json::json!({"first_name": "Jane"});
        assert_eq!(
            interpolate_title_template("{first_name} {last_name}", &props),
            "Jane"
        );
    }

    #[test]
    fn test_interpolate_null_field_is_empty() {
        let props = serde_json::json!({"first_name": "Alice", "email": null});
        assert_eq!(
            interpolate_title_template("{first_name} ({email})", &props),
            "Alice ()"
        );
    }

    #[test]
    fn test_interpolate_number_value() {
        let props = serde_json::json!({"invoice_number": 42});
        assert_eq!(
            interpolate_title_template("Invoice #{invoice_number}", &props),
            "Invoice #42"
        );
    }

    #[test]
    fn test_interpolate_boolean_value() {
        let props = serde_json::json!({"active": true});
        assert_eq!(
            interpolate_title_template("Active: {active}", &props),
            "Active: true"
        );
    }

    #[test]
    fn test_interpolate_multiple_fields() {
        let props = serde_json::json!({
            "first_name": "John",
            "last_name": "Doe",
            "email": "john@example.com"
        });
        assert_eq!(
            interpolate_title_template("{first_name} {last_name} ({email})", &props),
            "John Doe (john@example.com)"
        );
    }

    #[test]
    fn test_interpolate_trims_whitespace() {
        let props = serde_json::json!({"first_name": "Jane", "last_name": ""});
        assert_eq!(
            interpolate_title_template("{first_name} {last_name}", &props),
            "Jane"
        );
    }

    #[test]
    fn test_interpolate_no_placeholders() {
        let props = serde_json::json!({});
        assert_eq!(
            interpolate_title_template("Static Title", &props),
            "Static Title"
        );
    }

    #[test]
    fn test_interpolate_all_fields_missing() {
        let props = serde_json::json!({});
        assert_eq!(
            interpolate_title_template("{first_name} {last_name}", &props),
            ""
        );
    }

    // -------------------------------------------------------------------------
    // interpolate_title_template_with_schema tests
    // -------------------------------------------------------------------------

    fn make_status_field(core: &[(&str, &str)], user: &[(&str, &str)]) -> SchemaField {
        use crate::models::schema::EnumValue;
        SchemaField {
            name: "status".to_string(),
            field_type: "enum".to_string(),
            protection: crate::models::schema::SchemaProtectionLevel::Core,
            core_values: if core.is_empty() {
                None
            } else {
                Some(
                    core.iter()
                        .map(|(v, l)| EnumValue {
                            value: v.to_string(),
                            label: l.to_string(),
                        })
                        .collect(),
                )
            },
            user_values: if user.is_empty() {
                None
            } else {
                Some(
                    user.iter()
                        .map(|(v, l)| EnumValue {
                            value: v.to_string(),
                            label: l.to_string(),
                        })
                        .collect(),
                )
            },
            indexed: false,
            required: None,
            extensible: None,
            default: None,
            description: None,
            item_type: None,
            fields: None,
            item_fields: None,
        }
    }

    #[test]
    fn test_enum_label_resolved_from_core_values() {
        let fields = vec![make_status_field(
            &[("on_hold", "On Hold"), ("active", "Active")],
            &[],
        )];
        let props = serde_json::json!({"status": "on_hold"});
        assert_eq!(
            interpolate_title_template_with_schema("{status}", &props, &fields),
            "On Hold"
        );
    }

    #[test]
    fn test_enum_label_resolved_from_user_values() {
        let fields = vec![make_status_field(
            &[("active", "Active")],
            &[("custom_val", "Custom Label")],
        )];
        let props = serde_json::json!({"status": "custom_val"});
        assert_eq!(
            interpolate_title_template_with_schema("{status}", &props, &fields),
            "Custom Label"
        );
    }

    #[test]
    fn test_unknown_enum_value_falls_back_to_raw() {
        let fields = vec![make_status_field(&[("active", "Active")], &[])];
        let props = serde_json::json!({"status": "unknown_value"});
        assert_eq!(
            interpolate_title_template_with_schema("{status}", &props, &fields),
            "unknown_value"
        );
    }

    #[test]
    fn test_mixed_template_non_enum_and_enum() {
        use crate::models::schema::EnumValue;
        let name_field = SchemaField {
            name: "name".to_string(),
            field_type: "string".to_string(),
            protection: crate::models::schema::SchemaProtectionLevel::User,
            core_values: None,
            user_values: None,
            indexed: false,
            required: None,
            extensible: None,
            default: None,
            description: None,
            item_type: None,
            fields: None,
            item_fields: None,
        };
        let status_field = SchemaField {
            name: "status".to_string(),
            field_type: "enum".to_string(),
            protection: crate::models::schema::SchemaProtectionLevel::Core,
            core_values: Some(vec![
                EnumValue {
                    value: "on_hold".to_string(),
                    label: "On Hold".to_string(),
                },
                EnumValue {
                    value: "active".to_string(),
                    label: "Active".to_string(),
                },
            ]),
            user_values: None,
            indexed: false,
            required: None,
            extensible: None,
            default: None,
            description: None,
            item_type: None,
            fields: None,
            item_fields: None,
        };
        let fields = vec![name_field, status_field];
        let props = serde_json::json!({"name": "Data Migration", "status": "on_hold"});
        assert_eq!(
            interpolate_title_template_with_schema("{name} ({status})", &props, &fields),
            "Data Migration (On Hold)"
        );
    }

    #[test]
    fn test_missing_field_empty_with_schema() {
        let fields = vec![make_status_field(&[("active", "Active")], &[])];
        let props = serde_json::json!({});
        assert_eq!(
            interpolate_title_template_with_schema("{status}", &props, &fields),
            ""
        );
    }

    #[test]
    fn test_empty_fields_slice_behaves_like_basic() {
        let props = serde_json::json!({"status": "on_hold", "name": "Test"});
        assert_eq!(
            interpolate_title_template_with_schema("{name} {status}", &props, &[]),
            "Test on_hold"
        );
    }

    #[test]
    fn test_core_values_searched_before_user_values() {
        use crate::models::schema::EnumValue;
        // Same value in both core and user with different labels — core should win
        let field = SchemaField {
            name: "status".to_string(),
            field_type: "enum".to_string(),
            protection: crate::models::schema::SchemaProtectionLevel::Core,
            core_values: Some(vec![EnumValue {
                value: "shared".to_string(),
                label: "Core Label".to_string(),
            }]),
            user_values: Some(vec![EnumValue {
                value: "shared".to_string(),
                label: "User Label".to_string(),
            }]),
            indexed: false,
            required: None,
            extensible: None,
            default: None,
            description: None,
            item_type: None,
            fields: None,
            item_fields: None,
        };
        let props = serde_json::json!({"status": "shared"});
        assert_eq!(
            interpolate_title_template_with_schema("{status}", &props, &[field]),
            "Core Label"
        );
    }
}
