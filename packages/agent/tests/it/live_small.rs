//! Shared setup for the live tests that run on a small model on a CPU-only
//! machine: the model files, the thread count and the context size come from
//! the environment, and the GPU is switched off for the whole process.
//!
//! ```text
//! NODESPACE_FORCE_CPU=1                      no GPU backend takes part at all
//! NODESPACE_LIVE_CHAT_MODEL=<path.gguf>      the chat model
//! NODESPACE_LIVE_EMBED_MODEL=<path.gguf>     the embedding model
//! NODESPACE_LIVE_THREADS=4                   threads for both engines
//! NODESPACE_LIVE_N_CTX=16384                 chat context window
//! ```
//!
//! Point the two model variables at symlinks inside a scratch directory, not
//! at the files in the user's models directory: the integrity gate writes a
//! `<model>.verified.json` record beside the path it is given.

use std::sync::Arc;

use nodespace_nlp_engine::chat::ChatConfig;
use nodespace_nlp_engine::inference::local::LlamaChatInferenceEngine;
use nodespace_nlp_engine::ModelFamily;
use nodespace_nlp_engine::{EmbeddingConfig, EmbeddingService};

fn required(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| panic!("{var} must be set for a live small-model run"))
}

pub fn threads() -> i32 {
    std::env::var("NODESPACE_LIVE_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4)
}

/// Refuses to run unless the GPU is off, so a missing variable cannot put the
/// model on the GPU by default.
pub fn require_cpu_only() {
    assert!(
        nodespace_nlp_engine::config::cpu_only_requested(),
        "set NODESPACE_FORCE_CPU=1: these runs must not touch the GPU"
    );
}

pub fn load_embedder() -> Arc<EmbeddingService> {
    require_cpu_only();
    let config = EmbeddingConfig {
        model_path: Some(required("NODESPACE_LIVE_EMBED_MODEL").into()),
        n_gpu_layers: 0,
        n_threads: threads(),
        ..Default::default()
    };
    let mut nlp = EmbeddingService::new(config).expect("embedding config");
    nlp.initialize().expect("embedding model must load");
    Arc::new(nlp)
}

pub fn load_engine() -> Arc<LlamaChatInferenceEngine> {
    require_cpu_only();
    let path = required("NODESPACE_LIVE_CHAT_MODEL");
    let n_ctx = std::env::var("NODESPACE_LIVE_N_CTX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16_384);
    let config = ChatConfig {
        n_ctx,
        n_gpu_layers: 0,
        n_threads: threads(),
        ..Default::default()
    };
    Arc::new(
        LlamaChatInferenceEngine::load(
            &path,
            ModelFamily::Gemma4,
            config,
            nodespace_agent::local_agent::model_manager::expected_sha256_for_path(&path),
        )
        .expect("chat model must load"),
    )
}
