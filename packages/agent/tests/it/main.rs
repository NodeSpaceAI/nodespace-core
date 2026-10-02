//! The crate's integration tests, as one test binary.
//!
//! Every `tests/*.rs` file used to be its own binary, each linking the whole
//! dependency graph: ~100 links per gate run, and nextest starting each one to
//! list its tests. One binary per crate links once. nextest still runs every
//! test in its own process, so isolation between tests is unchanged. Add a
//! test file here as a module; run one file's tests with
//! `.tools/bin/cargo-nextest nextest run -p nodespace-agent --test it <module>::`.
//! Prefer nextest over a plain `cargo test` for anything but a single test:
//! `cargo test` runs the whole binary's tests as threads of one process.

mod conflict_tools;
mod decision_marker_golden;
mod extends_chain_tool_results_test;
mod get_related_nodes_both_direction;
mod golden_scenario6_handauthored;
mod golden_scenario6_sequence;
mod live_delete_resolved_incidents;
mod live_embedding_prefix_measurement;
mod live_openai_compat_routing;
mod live_openai_compat_smoke;
mod live_resolve_query_decomposition_shapes;
mod live_routing_probe;
mod live_skill_retrieval_stability;
mod live_stage1_compound_intent;
mod live_stage1_golden_prompts;
mod live_unset_schema_field;
mod live_update_node_noop_gate;
mod matrix_scenario_winnability;
mod prompt_assembly_snapshot;
mod routing_latency;
mod search_nodes_enumerate;
mod search_skills_latency;
mod seed_tables;
mod skill_guidance_fetch_mechanism;
