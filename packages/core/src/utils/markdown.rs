//! Markdown stripping utilities for title extraction
//!
//! This module provides functions to strip markdown formatting from content,
//! producing clean plain text suitable for search indexing and display.

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
}
