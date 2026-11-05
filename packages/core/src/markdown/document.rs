//! A markdown document as the title, metadata and body of a node.
//!
//! The first non-empty line of a document is its title and the rest is its
//! body, which the importer's parser turns into child nodes. A document may
//! begin with front matter: metadata about the document, held as properties
//! of its root rather than as nodes.

use serde_json::{Map, Value};

/// The namespaces an extension field may carry.
const EXTENSION_PREFIXES: [&str; 3] = ["custom:", "org:", "plugin:"];

/// The prefix a front matter key without a declared field is stored under.
const CUSTOM_PREFIX: &str = "custom:";

/// Front matter split off the start of a document.
#[derive(Debug, Clone, PartialEq)]
pub struct FrontMatter {
    /// The top-level keys of the block, each with the JSON type it has.
    pub fields: Map<String, Value>,
    /// The document after the block's closing line.
    pub body: String,
}

/// Split front matter off the start of `content`.
///
/// A block is front matter when the first line is exactly `---`, a later line
/// is exactly `---` or `...`, and the lines between them parse as a non-empty
/// YAML mapping. Anything else is not front matter and returns `None`, so a
/// `---` that does not meet the rule stays a horizontal line.
pub fn split_front_matter(content: &str) -> Option<FrontMatter> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.first().map(|l| l.trim_end_matches('\r')) != Some("---") {
        return None;
    }
    let close = lines
        .iter()
        .skip(1)
        .position(|l| matches!(l.trim_end_matches('\r'), "---" | "..."))?
        + 1;
    let block = lines[1..close].join("\n");
    let fields = match serde_yaml_ng::from_str::<Value>(&block) {
        Ok(Value::Object(fields)) if !fields.is_empty() => fields,
        _ => return None,
    };
    Some(FrontMatter {
        fields,
        body: lines[close + 1..].join("\n"),
    })
}

/// The properties a root takes from its front matter, ready to store.
///
/// - A key starting with `_` is reserved for bookkeeping and is left out.
/// - A key already in an extension namespace (`custom:`, `org:`, `plugin:`)
///   is stored as given.
/// - A key `is_declared` says the root's type declares is stored as that
///   field, so the type's own rules validate its value.
/// - Any other key is stored as `custom:<key>`: a core type takes only its
///   declared fields, and `custom:` is the namespace for one a user adds. A
///   key that names a core type is also stored that way, so it is never read
///   as that type's bucket.
pub fn front_matter_properties(
    fields: Map<String, Value>,
    is_declared: impl Fn(&str) -> bool,
) -> Map<String, Value> {
    let mut properties = Map::new();
    for (key, value) in fields {
        if key.starts_with('_') {
            continue;
        }
        let namespaced = EXTENSION_PREFIXES.iter().any(|p| key.starts_with(p));
        let declared = is_declared(&key) && crate::models::CoreNodeType::from_id(&key).is_none();
        let stored = if namespaced || declared {
            key
        } else {
            format!("{CUSTOM_PREFIX}{key}")
        };
        properties.insert(stored, value);
    }
    properties
}

/// The content of a document's root node: the plain title. A title taken from
/// the document's first line loses a leading Markdown heading marker (`#` to
/// `######` and the space after it); a filename-derived title is used as is.
pub fn root_content_for_title(title: &str, from_filename: bool) -> String {
    if from_filename {
        return title.to_string();
    }
    let trimmed = title.trim();
    let level = trimmed.bytes().take_while(|b| *b == b'#').count();
    match trimmed[level..].strip_prefix(' ') {
        Some(rest) if (1..=6).contains(&level) => rest.trim().to_string(),
        _ => trimmed.to_string(),
    }
}

/// The content the children are parsed from. When the title is the document's
/// first non-blank line, that line becomes the root and is dropped here; a
/// title taken from the filename leaves the content whole, since its first
/// line is body text even when it equals the filename stem.
pub fn content_without_title_line(content: &str, title: &str, title_from_filename: bool) -> String {
    let first_line = content.lines().find(|l| !l.trim().is_empty());
    if title_from_filename || first_line != Some(title) {
        return content.to_string();
    }
    let lines: Vec<&str> = content.lines().collect();
    let first_idx = lines.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
    lines[first_idx + 1..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn front_matter_keeps_each_value_as_the_json_type_it_has() {
        let doc = "---\nname: spec\ncount: 3\nratio: 0.5\ndraft: true\ntags: [a, b]\nowner:\n  team: core\n  size: 2\n---\n# Title\n";
        let fm = split_front_matter(doc).expect("front matter");
        assert_eq!(fm.fields["name"], json!("spec"));
        assert_eq!(fm.fields["count"], json!(3));
        assert_eq!(fm.fields["ratio"], json!(0.5));
        assert_eq!(fm.fields["draft"], json!(true));
        assert_eq!(fm.fields["tags"], json!(["a", "b"]));
        assert_eq!(fm.fields["owner"], json!({"team": "core", "size": 2}));
        assert_eq!(fm.body, "# Title");
    }

    #[test]
    fn a_closing_line_of_three_dots_ends_the_block() {
        let fm = split_front_matter("---\na: 1\n...\nbody").expect("front matter");
        assert_eq!(fm.fields["a"], json!(1));
        assert_eq!(fm.body, "body");
    }

    #[test]
    fn a_block_that_is_not_front_matter_is_none() {
        // Not the first line.
        assert!(split_front_matter("\n---\na: 1\n---\nx").is_none());
        assert!(split_front_matter("# T\n---\na: 1\n---\nx").is_none());
        // Never closed.
        assert!(split_front_matter("---\na: 1\nbody").is_none());
        // Not a mapping.
        assert!(split_front_matter("---\n- a\n- b\n---\nx").is_none());
        assert!(split_front_matter("---\njust text\n---\nx").is_none());
        // Empty.
        assert!(split_front_matter("---\n---\nx").is_none());
        // Unparseable.
        assert!(split_front_matter("---\na: [1\n---\nx").is_none());
        // Not exactly the marker.
        assert!(split_front_matter("--- \na: 1\n---\nx").is_none());
        assert!(split_front_matter("----\na: 1\n---\nx").is_none());
    }

    #[test]
    fn keys_are_stored_by_the_namespace_rule() {
        let fields = json!({
            "decision_status": "accepted",
            "author": "ann",
            "custom:x": 1,
            "org:y": 2,
            "_hidden": 3,
            "text": {"a": 1}
        });
        let Value::Object(fields) = fields else {
            unreachable!()
        };
        let props = front_matter_properties(fields, |k| k == "decision_status" || k == "text");
        assert_eq!(props["decision_status"], json!("accepted"));
        assert_eq!(props["custom:author"], json!("ann"));
        assert_eq!(props["custom:x"], json!(1));
        assert_eq!(props["org:y"], json!(2));
        assert!(!props.contains_key("_hidden"));
        assert!(!props.contains_key("custom:_hidden"));
        assert_eq!(props["custom:text"], json!({"a": 1}));
        assert_eq!(props.len(), 6 - 1);
    }

    #[test]
    fn the_title_loses_its_heading_marker() {
        for first_line in ["# Title", "## Title", "###### Title", "Title"] {
            assert_eq!(root_content_for_title(first_line, false), "Title");
        }
        assert_eq!(root_content_for_title("# C# notes", true), "# C# notes");
    }
}
