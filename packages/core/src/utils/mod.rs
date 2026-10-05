//! Utility functions for NodeSpace Core
//!
//! This module provides common utility functions used across the codebase.

mod markdown;

pub use markdown::{interpolate_title_template_with_schema, strip_markdown, title_template_fields};

/// `noun` behind its indefinite article: "a task", "an issue".
///
/// Chosen by the first letter, which is right for a type name (the nouns this
/// is for): a lowercase key such as `ai-chat-message`. It does not know words
/// whose sound differs from their spelling ("an hour", "a user").
pub fn with_indefinite_article(noun: &str) -> String {
    let starts_with_vowel = noun
        .chars()
        .next()
        .is_some_and(|c| matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u'));
    let article = if starts_with_vowel { "an" } else { "a" };
    format!("{article} {noun}")
}

#[cfg(test)]
mod tests {
    use super::with_indefinite_article;

    #[test]
    fn the_article_follows_the_first_letter() {
        assert_eq!(with_indefinite_article("task"), "a task");
        assert_eq!(
            with_indefinite_article("ai-chat-message"),
            "an ai-chat-message"
        );
        assert_eq!(with_indefinite_article("Issue"), "an Issue");
    }
}
