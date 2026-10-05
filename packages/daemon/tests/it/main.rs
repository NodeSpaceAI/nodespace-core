//! The crate's integration tests, as one test binary.
//!
//! Every `tests/*.rs` file used to be its own binary, each linking the whole
//! dependency graph: ~100 links per gate run, and nextest starting each one to
//! list its tests. One binary per crate links once. nextest still runs every
//! test in its own process, so isolation between tests is unchanged. Add a
//! test file here as a module; run one file's tests with
//! `.tools/bin/cargo-nextest nextest run -p nodespace-daemon --test it <module>::`.
//! Prefer nextest over a plain `cargo test` for anything but a single test:
//! `cargo test` runs the whole binary's tests as threads of one process.

mod agent_session_grpc;
mod database_service_e2e;
mod extension_points_fixture;
mod golden_reassignment_restore_real_pipeline;
mod golden_scenario6_real_pipeline;
mod grpc_round_trip;
mod import_round_trip;
mod live_terminal_summary;
mod per_db_compute;
mod per_db_subtree_gate;
mod required_extensions_e2e;
mod scenario6_update_node_properties;
mod sigterm_during_model_load;
mod single_instance;
mod unrecognized_flag;
