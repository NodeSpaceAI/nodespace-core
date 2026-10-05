//! What another build adds to core's data (ADR-082 §2, ADR-083 §2).
//!
//! An extension is named by an id: the entry a database lists in its settings
//! node's `required_extensions`, and the bucket its fields take in a core
//! relationship's edge properties.

/// Whether `id` has the form of an extension id: a lowercase ASCII letter,
/// then lowercase ASCII letters, digits, `-` or `_`.
///
/// ```
/// use nodespace_core::extensions::is_extension_id;
///
/// assert!(is_extension_id("fixture"));
/// assert!(!is_extension_id("Fixture"));
/// ```
pub fn is_extension_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extension_id_is_a_lowercase_letter_then_letters_digits_dashes_or_underscores() {
        for valid in ["fixture", "ext", "a", "x2", "my-ext", "my_ext"] {
            assert!(is_extension_id(valid), "{valid:?} is an extension id");
        }
        for invalid in ["", "Ext", "2x", "-x", "_x", "my ext", "my.ext", "é"] {
            assert!(
                !is_extension_id(invalid),
                "{invalid:?} is not an extension id"
            );
        }
    }
}
