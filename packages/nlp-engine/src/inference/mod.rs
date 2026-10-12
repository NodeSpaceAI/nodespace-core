//! Inference engines: every way a conversation reaches a model.
//!
//! The interface ([`ChatInferenceEngine`]) and its request and response types
//! live in [`engine`]. Two engines implement it and are interchangeable to
//! everything above this crate:
//!
//! - [`local`] — the llama.cpp engine, bridged from [`crate::chat::ChatEngine`];
//! - [`openai_compat`] — the client for user-configured OpenAI-compatible
//!   endpoints, with its model discovery in [`openai_compat_discovery`].
//!
//! [`prompt_dump`] is the one dev-only dump both write to.
//!
//! The remote client needs no llama.cpp, so this module compiles with the
//! chat feature off: a build with no local engine can still reach a remote
//! model.

pub mod engine;
pub mod local;
pub mod model;
pub mod ndjson;
pub mod openai_compat;
pub mod openai_compat_discovery;
pub mod prompt_dump;
