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

/// What the start of a document holds.
#[derive(Debug, Clone, PartialEq)]
pub enum FrontMatterBlock {
    /// The document does not start with a `---` line closed by a later `---`
    /// or `...` line.
    Absent,
    /// A block that parses as a non-empty YAML mapping with string keys.
    Parsed(FrontMatter),
    /// The first line is `---` and a closing line follows, but what is between
    /// them is not a non-empty YAML mapping with unique string keys (a YAML
    /// error, a list, plain text, a non-string or repeated key).
    Malformed,
}

/// Read the front matter block at the start of `content`.
pub fn parse_front_matter(content: &str) -> FrontMatterBlock {
    let lines: Vec<&str> = content.lines().collect();
    if lines.first().map(|l| l.trim_end_matches('\r')) != Some("---") {
        return FrontMatterBlock::Absent;
    }
    let Some(close) = lines
        .iter()
        .skip(1)
        .position(|l| matches!(l.trim_end_matches('\r'), "---" | "..."))
        .map(|i| i + 1)
    else {
        return FrontMatterBlock::Absent;
    };
    let block = lines[1..close].join("\n");
    // Read as YAML first: a mapping with a repeated key is an error there,
    // where reading straight into JSON would keep the last one. JSON needs
    // every key to be a string.
    let fields = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&block)
        .ok()
        .filter(|yaml| {
            yaml.as_mapping()
                .is_some_and(|map| map.keys().all(serde_yaml_ng::Value::is_string))
        })
        .and_then(|yaml| serde_json::to_value(yaml).ok());
    match fields {
        Some(Value::Object(fields)) if !fields.is_empty() => {
            FrontMatterBlock::Parsed(FrontMatter {
                fields,
                body: lines[close + 1..].join("\n"),
            })
        }
        _ => FrontMatterBlock::Malformed,
    }
}

/// Split front matter off the start of `content`.
///
/// A block is front matter when the first line is exactly `---`, a later line
/// is exactly `---` or `...`, and the lines between them parse as a non-empty
/// YAML mapping. Anything else returns `None`, so a `---` that does not meet
/// the rule stays a horizontal line (the importer's rule; see
/// [`parse_front_matter`] for a caller that refuses a malformed block).
pub fn split_front_matter(content: &str) -> Option<FrontMatter> {
    match parse_front_matter(content) {
        FrontMatterBlock::Parsed(front) => Some(front),
        _ => None,
    }
}

/// Why `line` cannot be the title of a document, or `None` when it can: a
/// title is a plain line or a heading. A line that opens a list, quote, code
/// fence, table, rule or checklist item would be a fragment of that block once
/// taken out of its document.
pub fn title_line_problem(line: &str) -> Option<&'static str> {
    let trimmed = line.trim();
    if trimmed.starts_with('|') {
        return Some("a table row");
    }
    let parsed = match super::prepare_nodes_from_markdown(trimmed, None) {
        Ok(nodes) => nodes,
        Err(_) => return Some("not valid markdown"),
    };
    match parsed.as_slice() {
        [node]
            if matches!(node.node_type.as_str(), "text" | "header") && node.content == trimmed =>
        {
            None
        }
        [node] => Some(match node.node_type.as_str() {
            "code-block" => "a code fence",
            "quote-block" => "a quote",
            "ordered-list" => "a numbered list",
            "horizontal-line" => "a horizontal rule",
            "checkbox" => "a checklist item",
            _ => "a list item",
        }),
        _ => Some("more than one block"),
    }
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
        // A prefix with nothing after it names no field, so it is not a
        // namespace: the key is stored under `custom:` like any other.
        let namespaced = EXTENSION_PREFIXES
            .iter()
            .any(|p| key.strip_prefix(p).is_some_and(|rest| !rest.is_empty()));
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
    fn a_closed_block_that_is_not_a_mapping_is_malformed() {
        for doc in [
            "---\n1: a\n---\n# T",
            "---\na: 1\na: 2\n---\n# T",
            "---\na: [1\n---\n# T",
            "---\n- a\n---\n# T",
            "---\njust text\n---\n# T",
            "---\n---\n# T",
        ] {
            assert_eq!(
                parse_front_matter(doc),
                FrontMatterBlock::Malformed,
                "{doc:?}"
            );
        }
        assert_eq!(
            parse_front_matter("---\na: 1\n# T"),
            FrontMatterBlock::Absent
        );
        assert_eq!(
            parse_front_matter("# T\n---\na: 1\n---"),
            FrontMatterBlock::Absent
        );
    }

    #[test]
    fn only_a_plain_line_or_a_heading_can_be_a_title() {
        for ok in ["Title", "# Title", "## Title", "A sentence, with commas."] {
            assert_eq!(title_line_problem(ok), None, "{ok:?}");
        }
        for bad in [
            "```rust",
            "| a | b |",
            "1. x",
            "> q",
            "- a",
            "- [ ] t",
            "---",
        ] {
            assert!(title_line_problem(bad).is_some(), "{bad:?}");
        }
    }

    #[test]
    fn an_empty_namespace_is_not_a_namespace() {
        let Value::Object(fields) = json!({"custom:": 1, "plugin:": 2, "org:x": 3}) else {
            unreachable!()
        };
        let props = front_matter_properties(fields, |_| false);
        assert_eq!(props["custom:custom:"], json!(1));
        assert_eq!(props["custom:plugin:"], json!(2));
        assert_eq!(props["org:x"], json!(3));
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
