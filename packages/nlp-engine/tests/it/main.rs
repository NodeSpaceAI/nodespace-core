//! The crate's integration tests, as one test binary.
//!
//! Every `tests/*.rs` file used to be its own binary, each linking the whole
//! dependency graph: ~100 links per gate run, and nextest starting each one to
//! list its tests. One binary per crate links once. nextest still runs every
//! test in its own process, so isolation between tests is unchanged. Add a
//! test file here as a module; run one file's tests with
//! `cargo test -p nodespace-nlp-engine --test it <module>::`.

mod embedding_integration;
mod toolcall_json_shape;
