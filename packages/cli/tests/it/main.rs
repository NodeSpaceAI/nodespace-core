//! The crate's integration tests, as one test binary.
//!
//! Every `tests/*.rs` file used to be its own binary, each linking the whole
//! dependency graph: ~100 links per gate run, and nextest starting each one to
//! list its tests. One binary per crate links once. nextest still runs every
//! test in its own process, so isolation between tests is unchanged. Add a
//! test file here as a module; run one file's tests with
//! `.tools/bin/cargo-nextest nextest run -p nodespace-cli --test it <module>::`.
//! Prefer nextest over a plain `cargo test` for anything but a single test:
//! `cargo test` runs the whole binary's tests as threads of one process.

mod cli_integration;
mod import_dir_multi;
mod mcp_integration;
mod skill_md_generation;
