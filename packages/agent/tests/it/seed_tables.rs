//! The seed tables, taken together.
//!
//! Every node NodeSpace seeds is defined by a table row with a fixed literal
//! UUID (ADR-086 §10): agent guidance, the built-in skills and tools, and the
//! core plays. A seeded node's identity is its id, so the ids must be real node
//! ids and no two rows anywhere may share one. The tables live in two crates;
//! this is the one place that sees them all.

use std::collections::HashMap;

use nodespace_agent::local_agent::tools::Tool;
use nodespace_agent::prompt_assembler::{PromptAssembler, GUIDANCE_SEEDS};
use nodespace_agent::skill_pipeline::{seed_skill_nodes, seed_tool_nodes, SKILL_SEEDS};
use nodespace_core::playbook::core_plays::CORE_PLAY_IDS;
use nodespace_core::services::node_service::is_valid_node_id;

/// Every seeded id, labelled with the table and row it comes from.
fn every_seed_id() -> Vec<(String, String)> {
    let mut ids = Vec::new();
    let mut push = |table: &str, row: &str, id: &str| {
        ids.push((format!("{table}: {row}"), id.to_string()));
    };

    for seed in GUIDANCE_SEEDS {
        push("agent guidance", seed.title, seed.id);
    }
    for seed in SKILL_SEEDS {
        push("skills", seed.title, seed.id);
    }
    for tool in Tool::ALL {
        push("tools", tool.name(), tool.seed_id());
    }
    for id in CORE_PLAY_IDS {
        push("core plays", id, id);
    }
    ids
}

#[test]
fn every_seeded_id_is_a_uuid() {
    let ids = every_seed_id();
    for table in ["agent guidance", "skills", "tools", "core plays"] {
        assert!(
            ids.iter().any(|(row, _)| row.starts_with(table)),
            "expected the {table} table to contribute ids"
        );
    }
    for (row, id) in ids {
        assert!(
            uuid::Uuid::parse_str(&id).is_ok(),
            "{row} has id {id:?}, which is not a UUID"
        );
        assert_eq!(id.len(), 36, "{row}: {id:?} is not in hyphenated form");
        assert_eq!(id, id.to_lowercase(), "{row}: {id:?} is not lowercase");
        assert!(
            is_valid_node_id(&id),
            "{row} has id {id:?}, which the create path would reject"
        );
    }
}

#[test]
fn no_two_seeds_share_an_id_across_any_table() {
    let mut seen: HashMap<String, String> = HashMap::new();
    for (row, id) in every_seed_id() {
        if let Some(other) = seen.insert(id.clone(), row.clone()) {
            panic!("{id} is the id of both `{other}` and `{row}`");
        }
    }
}

/// What the daemon seeds is the tables, row for row: each seed function
/// yields its table's ids in table order.
#[test]
fn the_seed_functions_yield_their_tables_in_order() {
    let ids = |templates: Vec<nodespace_core::markdown::NodeTemplate>| -> Vec<String> {
        templates.into_iter().map(|t| t.id).collect()
    };
    assert_eq!(
        ids(PromptAssembler::seed_agent_guidance_nodes()),
        GUIDANCE_SEEDS.iter().map(|s| s.id).collect::<Vec<_>>()
    );
    assert_eq!(
        ids(seed_skill_nodes()),
        SKILL_SEEDS.iter().map(|s| s.id).collect::<Vec<_>>()
    );
    assert_eq!(
        ids(seed_tool_nodes()),
        Tool::ALL.iter().map(|t| t.seed_id()).collect::<Vec<_>>()
    );
}
