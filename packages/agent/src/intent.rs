//! Intent extraction via pattern matching and filler stripping.
//!
//! Extracts user intent from messages for skill discovery. Uses keyword
//! pattern matching first (zero latency), falls back to filler-stripped
//! first sentence as a raw semantic query.
//!
//! Issue #1050, ADR-030 Phase 3.

/// Result of intent extraction from a user message.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedIntent {
    /// The extracted intent string (either a canonical intent or cleaned query)
    pub query: String,
    /// Whether this came from a pattern match (vs filler-stripped fallback)
    pub from_pattern: bool,
}

/// Intent patterns: keyword/phrase → canonical intent mapping.
///
/// Order matters: more specific patterns should come first.
static INTENT_PATTERNS: &[(&[&str], &str)] = &[
    // Deduplication / similarity analysis
    (
        &[
            "find duplicates",
            "overlap",
            "similar to each other",
            "deduplicate",
            "dedup",
        ],
        "deduplicate",
    ),
    // Search & discovery
    (
        &[
            "search for",
            "find",
            "look up",
            "what do i know about",
            "look for",
            "where is",
            "show me",
        ],
        "search",
    ),
    // Schema/type creation (before general creation to match first)
    (
        &[
            "new type",
            "new node type",
            "a new node type",
            "new schema",
            "new entity type",
            "define a type",
            "create a type",
            "create a schema",
            "create a new type",
            "create a new node type",
            "create a new schema",
            "define fields",
            "define a schema",
            "node type",
            "with fields",
            "entity type",
            "schema for",
        ],
        "create schema",
    ),
    // Import / bulk creation (before general creation to match first)
    (
        &["create nodes from", "bulk create", "bulk import", "import"],
        "import",
    ),
    // Creation
    (
        &["create", "make", "add a new", "new node", "add a"],
        "create",
    ),
    // Updates
    (
        &[
            "update", "change", "modify", "edit", "rename", "set", "mark",
        ],
        "update",
    ),
    // Relationships / connections
    (&["relate", "connect", "link", "associate"], "relate"),
    // Summarization
    (
        &["summarize", "overview", "recap", "summary of", "brief on"],
        "summarize",
    ),
    // Deletion
    (&["delete", "remove", "trash", "archive"], "delete"),
    // Organization
    (
        &["organize", "categorize", "sort", "group", "tag"],
        "organize",
    ),
];

/// Conversational filler words/phrases to strip from fallback queries.
static FILLER_PATTERNS: &[&str] = &[
    "can you",
    "could you",
    "would you",
    "please",
    "i want to",
    "i need to",
    "i'd like to",
    "help me",
    "let's",
    "go ahead and",
    "i want you to",
    "i need you to",
    "try to",
    "just",
    "hey",
    "hi",
    "hello",
    "okay",
    "ok",
    "so",
    "well",
    "um",
    "uh",
    "actually",
    "basically",
    "honestly",
    "literally",
];

/// Extract intent from a user message.
///
/// 1. Pattern match: check known intent patterns
/// 2. Fallback: strip filler words, take first sentence
pub fn extract_intent(message: &str) -> ExtractedIntent {
    let lower = message.to_lowercase();

    // Phase 1: Pattern matching (specific → general)
    for (patterns, intent) in INTENT_PATTERNS {
        for pattern in *patterns {
            if lower.contains(pattern) {
                return ExtractedIntent {
                    query: intent.to_string(),
                    from_pattern: true,
                };
            }
        }
    }

    // Phase 2: Filler-stripped fallback
    let cleaned = strip_filler(&lower);
    let first_sentence = cleaned
        .split(&['.', '?', '!'][..])
        .next()
        .unwrap_or(&cleaned)
        .trim()
        .to_string();

    ExtractedIntent {
        query: if first_sentence.is_empty() {
            lower.trim().to_string()
        } else {
            first_sentence
        },
        from_pattern: false,
    }
}

/// Strip conversational filler from the start of a message.
///
/// Loops until stable: removing one filler may reveal another.
fn strip_filler(message: &str) -> String {
    let mut result = message.to_string();
    loop {
        let before = result.clone();
        for filler in FILLER_PATTERNS {
            if filler.is_empty() {
                continue;
            }
            while result.trim_start().starts_with(filler) {
                let trimmed = result.trim_start();
                result = trimmed[filler.len()..].to_string();
            }
        }
        if result == before {
            break;
        }
    }
    // Collapse multiple spaces
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Pattern matching tests ---

    #[test]
    fn extract_search_intent() {
        let result = extract_intent("search for invoices from last month");
        assert_eq!(result.query, "search");
        assert!(result.from_pattern);
    }

    #[test]
    fn extract_search_what_do_i_know() {
        let result = extract_intent("What do I know about machine learning?");
        assert_eq!(result.query, "search");
        assert!(result.from_pattern);
    }

    #[test]
    fn extract_create_intent() {
        let result = extract_intent("Create a new customer node for Acme Corp");
        assert_eq!(result.query, "create");
        assert!(result.from_pattern);
    }

    #[test]
    fn extract_update_intent() {
        let result = extract_intent("Update the status of task #42 to done");
        assert_eq!(result.query, "update");
        assert!(result.from_pattern);
    }

    #[test]
    fn extract_relate_intent() {
        let result = extract_intent("Connect the invoice to the customer");
        assert_eq!(result.query, "relate");
        assert!(result.from_pattern);
    }

    #[test]
    fn extract_summarize_intent() {
        let result = extract_intent("Give me a summary of the project notes");
        assert_eq!(result.query, "summarize");
        assert!(result.from_pattern);
    }

    #[test]
    fn extract_deduplicate_intent() {
        let result = extract_intent("Find duplicates in my contact list");
        assert_eq!(result.query, "deduplicate");
        assert!(result.from_pattern);
    }

    #[test]
    fn extract_delete_intent() {
        let result = extract_intent("Delete the old meeting notes");
        assert_eq!(result.query, "delete");
        assert!(result.from_pattern);
    }

    #[test]
    fn extract_organize_intent() {
        let result = extract_intent("Organize my notes by category");
        assert_eq!(result.query, "organize");
        assert!(result.from_pattern);
    }

    // --- Case insensitivity ---

    #[test]
    fn case_insensitive_matching() {
        let result = extract_intent("SEARCH FOR important documents");
        assert_eq!(result.query, "search");
        assert!(result.from_pattern);
    }

    // --- Filler stripping fallback ---

    #[test]
    fn fallback_strips_filler() {
        let result = extract_intent("Hey, can you help me with the billing architecture?");
        assert!(!result.from_pattern);
        // Should strip filler and return meaningful part
        assert!(!result.query.contains("hey"));
        assert!(result.query.contains("billing") || result.query.contains("architecture"));
    }

    #[test]
    fn fallback_takes_first_sentence() {
        let result = extract_intent("Tell me about the revenue model. Also check the metrics.");
        assert!(!result.from_pattern);
        assert!(result.query.contains("revenue model"));
        assert!(!result.query.contains("metrics"));
    }

    #[test]
    fn fallback_handles_empty_after_strip() {
        // Even with heavy filler, should return something
        let result = extract_intent("Hey hello");
        assert!(!result.query.is_empty());
    }

    #[test]
    fn fallback_no_filler_passthrough() {
        let result = extract_intent("What are the quarterly results?");
        assert!(!result.from_pattern);
        assert!(result.query.contains("quarterly results"));
    }

    // --- Import intent ---

    #[test]
    fn extract_import_intent() {
        let result = extract_intent("Import this document into NodeSpace");
        assert_eq!(result.query, "import");
        assert!(result.from_pattern);
    }

    #[test]
    fn extract_bulk_import_intent() {
        let result = extract_intent("Bulk import these records");
        assert_eq!(result.query, "import");
        assert!(result.from_pattern);
    }

    #[test]
    fn extract_create_nodes_from_intent() {
        let result = extract_intent("Create nodes from this markdown document");
        assert_eq!(result.query, "import");
        assert!(result.from_pattern);
    }

    // --- Skill pipeline intent coverage (all 8 skills) ---

    #[test]
    fn skill_research_search_intent() {
        let result = extract_intent("What do I know about machine learning?");
        assert_eq!(result.query, "search");
        assert!(result.from_pattern);
    }

    #[test]
    fn skill_node_creation_intent() {
        let result = extract_intent("Create a new task: Do laundry");
        assert_eq!(result.query, "create");
        assert!(result.from_pattern);
    }

    #[test]
    fn skill_schema_creation_intent() {
        let result = extract_intent("Create a new type Project with fields for tracking");
        assert_eq!(result.query, "create schema");
        assert!(result.from_pattern);
    }

    #[test]
    fn skill_graph_editing_intent() {
        let result = extract_intent("Mark task X as done");
        assert_eq!(result.query, "update");
        assert!(result.from_pattern);
    }

    #[test]
    fn skill_relationship_management_intent() {
        let result = extract_intent("Connect invoice to customer");
        assert_eq!(result.query, "relate");
        assert!(result.from_pattern);
    }

    #[test]
    fn skill_node_deletion_intent() {
        let result = extract_intent("Delete the old meeting notes");
        assert_eq!(result.query, "delete");
        assert!(result.from_pattern);
    }

    #[test]
    fn skill_bulk_import_intent() {
        let result = extract_intent("Import this document into my knowledge base");
        assert_eq!(result.query, "import");
        assert!(result.from_pattern);
    }

    #[test]
    fn skill_organization_intent() {
        let result = extract_intent("Add to collection X");
        // "add to" doesn't match a specific pattern, fallback triggers
        // but "organize" would be matched if present
        let result2 = extract_intent("Organize my notes into categories");
        assert_eq!(result2.query, "organize");
        assert!(result2.from_pattern);
        // "add to collection" - fallback (no explicit pattern match)
        assert!(!result.query.is_empty());
    }

    // --- Edge cases ---

    #[test]
    fn empty_message() {
        let result = extract_intent("");
        assert!(!result.from_pattern);
        assert!(result.query.is_empty());
    }

    #[test]
    fn whitespace_only_message() {
        let result = extract_intent("   ");
        assert!(!result.from_pattern);
    }

    #[test]
    fn first_pattern_match_wins() {
        // "find duplicates" should match deduplicate, not search (even though "find" matches search)
        let result = extract_intent("find duplicates across all notes");
        assert_eq!(result.query, "deduplicate");
    }

    #[test]
    fn add_a_matches_create() {
        let result = extract_intent("Add a new task for the weekly review");
        assert_eq!(result.query, "create");
        assert!(result.from_pattern);
    }

    // --- Schema creation intent ---

    #[test]
    fn create_new_type_matches_schema() {
        let result =
            extract_intent("Create a new type 'Project' and define fields for tracking projects");
        assert_eq!(result.query, "create schema");
        assert!(result.from_pattern);
    }

    #[test]
    fn create_new_node_type_with_colon_matches_schema() {
        // Regression: "Create a new node type: 'Project'" was matching "create" instead of schema
        let result = extract_intent(
            "Create a new node type: 'Project' and use fields that are typical of what we want to track on a project",
        );
        assert_eq!(result.query, "create schema");
        assert!(result.from_pattern);
    }

    #[test]
    fn entity_type_matches_schema() {
        let result = extract_intent("I need a new entity type for invoices");
        assert_eq!(result.query, "create schema");
        assert!(result.from_pattern);
    }

    #[test]
    fn schema_for_matches_schema() {
        let result = extract_intent("Create a schema for tracking customer orders");
        assert_eq!(result.query, "create schema");
        assert!(result.from_pattern);
    }

    #[test]
    fn define_schema_matches_schema() {
        let result = extract_intent("Define a schema for customer records");
        assert_eq!(result.query, "create schema");
        assert!(result.from_pattern);
    }

    #[test]
    fn new_entity_type_matches_schema() {
        let result = extract_intent("I need a new entity type for invoices");
        assert_eq!(result.query, "create schema");
        assert!(result.from_pattern);
    }

    #[test]
    fn define_fields_matches_schema() {
        let result = extract_intent("Define fields for the employee type");
        assert_eq!(result.query, "create schema");
        assert!(result.from_pattern);
    }
}
