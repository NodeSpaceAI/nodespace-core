//! Live-model check that a conversation may open with the assistant's
//! message, against the real Gemma 4 E4B chat template.
//!
//! A chat the system opens (ADR-090 §3) sends the model `system, assistant,
//! system, user` on its first turn, where every other chat starts with the
//! user. The template is the model's own, applied by llama.cpp, and some
//! templates refuse a conversation whose roles do not alternate from the
//! user. A stub engine renders no template, so this is the only place that
//! shape is shown to work.
//!
//! Ignored by default: it needs the E4B GGUF on disk and a few seconds of GPU
//! time. Run explicitly with `--ignored --nocapture`.

#![cfg(feature = "chat-service")]

use nodespace_nlp_engine::{ChatChunk, ChatConfig, ChatEngine, ChatMessage, Role};
use std::sync::{Arc, Mutex};

fn model_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    std::path::PathBuf::from(home).join(".nodespace/models/gemma-4-E4B-it-Q4_K_M.gguf")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the Gemma 4 E4B GGUF; run explicitly with --ignored --nocapture"]
async fn a_conversation_opened_by_the_assistant_renders_and_is_answered() {
    let path = model_path();
    if !path.exists() {
        eprintln!("SKIP: model not found at {}", path.display());
        return;
    }

    let service = ChatEngine::new(ChatConfig {
        // The window the engine requires; at Q8_0 it fits beside the weights
        // on a 16 GB machine.
        n_ctx: 16384,
        n_gpu_layers: 99,
        type_k: Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0),
        type_v: Some(nodespace_nlp_engine::KvCacheQuantType::Q8_0),
        ..Default::default()
    })
    .expect("service construction");
    service
        .load_model(path.to_str().expect("model path is utf-8"), None)
        .expect("model load");

    let reply = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&reply);
    service
        .generate_streaming(
            vec![
                ChatMessage::text(Role::System, "You help the user change an automation."),
                ChatMessage::text(
                    Role::Assistant,
                    "What would you like to change in [Close finished parents](nodespace://p1)?",
                ),
                ChatMessage::text(
                    Role::System,
                    "Nodes pinned to this conversation. The conversation is about them:\n\
                     - nodespace://p1 \"Close finished parents\" (play)",
                ),
                ChatMessage::text(Role::User, "Change a condition"),
            ],
            None,
            0.0,
            128,
            move |chunk| {
                if let ChatChunk::Token(text) = chunk {
                    sink.lock().unwrap().push_str(&text);
                }
            },
        )
        .await
        .expect("an assistant-first conversation must render and generate");

    let reply = reply.lock().unwrap().clone();
    println!("reply: {reply}");
    assert!(
        !reply.trim().is_empty(),
        "the model must answer the user's reply to the opening message"
    );
}
