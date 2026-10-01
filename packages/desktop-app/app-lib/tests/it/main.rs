//! The crate's integration tests, as one test binary.
//!
//! Every `tests/*.rs` file used to be its own binary, each linking the whole
//! dependency graph: ~100 links per gate run, and nextest starting each one to
//! list its tests. One binary per crate links once. nextest still runs every
//! test in its own process, so isolation between tests is unchanged. Add a
//! test file here as a module; run one file's tests with
//! `.tools/bin/cargo-nextest nextest run -p nodespace-app-lib --test it <module>::`.
//! Prefer nextest over a plain `cargo test` for anything but a single test:
//! `cargo test` runs the whole binary's tests as threads of one process.

mod adapter_contract_test;
mod ai_chat_send_to_idle_test;
mod commits_per_edit_test;
mod cross_window_hierarchy_sync_test;
mod daemon_binary_freshness_test;
mod daemon_custom_socket_test;
mod daemon_readiness_test;
mod extension_hooks_test;
mod indent_outdent_rapid_ordering_test;
mod model_download_terminal_state_test;
mod node_crud_tauri_seam_test;
mod optimistic_echo_race_test;
mod out_of_band_write_echo_test;
mod schema_parity_test;
mod sidecar_staging_sync_test;
mod startup_readiness_data_plane_test;
mod window_routing_no_cross_talk_test;
