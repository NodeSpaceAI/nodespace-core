//! The crate's integration tests, as one test binary.
//!
//! Every `tests/*.rs` file used to be its own binary, each linking the whole
//! dependency graph: ~100 links per gate run, and nextest starting each one to
//! list its tests. One binary per crate links once. nextest still runs every
//! test in its own process, so isolation between tests is unchanged. Add a
//! test file here as a module; run one file's tests with
//! `.tools/bin/cargo-nextest nextest run -p nodespace-core --test it <module>::`.
//! Prefer nextest over a plain `cargo test` for anything but a single test:
//! `cargo test` runs the whole binary's tests as threads of one process.

mod ai_chat_message_test;
mod ai_chat_reference_test;
mod ai_chat_subtypes_test;
mod array_field_type_validation_test;
mod bulk_invariant_dispatch_test;
mod collection_membership_test;
mod collection_name_convergence_test;
mod concurrent_schema_creation_test;
mod conflict_reconciliation_test;
mod context_paths_test;
mod core_context_paths_seed_test;
mod core_model_plays_test;
mod core_queries_test;
mod core_task_link_relationships_test;
mod core_type_registry_test;
mod create_node_property_persistence_test;
mod create_schema_result_reflects_persisted_state_test;
mod database_settings_fields_test;
mod default_scope_schema_search_live_test;
mod derived_attributes_test;
mod embedding_service_test;
mod entity_resolution_test;
mod entity_type_descriptor_extends_chain_test;
mod event_emission_test;
mod extends_subtype_identity_test;
mod find_skills_schema_discovery_test;
mod find_skills_schema_metadata_test;
mod kebab_schema_id_test;
mod link_field_type_test;
mod mentioning_containers_test;
mod merge_nodes_test;
mod move_node_ordering_test;
mod node_completeness_extends_chain_test;
mod node_ops_extends_chain_test;
mod object_field_type_validation_test;
mod object_valued_property_roundtrip_test;
mod parent_task_completion_play_test;
mod participation_test;
mod pending_seed_updates_test;
mod permitted_filter_test;
mod person_duplicate_convergence_test;
mod person_seed_test;
mod playbook_engine_integration_test;
mod playbook_invariant_integration_test;
mod query_service_test;
mod reimport_store_primitives_test;
mod rel_ops_forward_name_extends_chain_test;
mod relationship_editing_test;
mod relationship_extends_chain_test;
mod relationship_in_name_normalization_test;
mod relationship_reverse_name_traversal_test;
mod required_extensions_field_test;
mod reverse_relationship_name_test;
mod scalar_field_type_validation_test;
mod schema_chain_blindness_guard_test;
mod schema_embedding_queue_test;
mod schema_hydration_test;
mod schema_relationship_declarations_test;
mod schema_test;
mod search_result_scaling_test;
mod seeded_links_test;
mod sibling_order_rebalance_test;
mod skill_applies_to_test;
mod skill_attached_to_test;
mod store_concurrency_test;
mod structural_rules_test;
mod tool_subtypes_test;
mod typed_flat_updates_test;
mod unique_field_extends_chain_test;
mod update_node_transaction_test;
