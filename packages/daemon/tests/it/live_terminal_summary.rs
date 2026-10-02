//! Live check that the locked native model turns a terminal session's output
//! into a usable summary.
//!
//! The deterministic tests in `terminal_summary.rs` pin what is enforced on the
//! model's reply whatever it writes (no escape sequences, no copied output).
//! They cannot show that the model's reply is worth storing, or that the copy
//! check does not discard every honest summary. That takes the real model.
//!
//! Live run:
//! ```text
//! .tools/bin/cargo-nextest nextest run -p nodespace-daemon --test it live_terminal_summary:: --run-ignored only --no-capture
//! ```

use std::sync::Arc;

use chrono::Utc;
use nodespace_agent::agent_types::{ChatInferenceEngine, ModelFamily};
use nodespace_agent::local_agent::inference::LlamaChatInferenceEngine;
use nodespace_agent::pty::{OutputChunk, SessionCapture};
use nodespace_daemon::services::terminal_summary::{copied_from_output, generate_summary};
use nodespace_nlp_engine::chat::ChatConfig;

/// Independent repetitions per session. Single runs on this stack are not
/// decision-grade.
const REPS: usize = 3;

/// Production's enforced floor. The prompt is a few thousand characters, and
/// a larger window only competes for GPU memory with a resident daemon.
const LIVE_N_CTX: u32 = 16_384;

fn model_path() -> String {
    let home = std::env::var("HOME").expect("HOME must be set");
    format!("{home}/.nodespace/models/gemma-4-E4B-it-Q4_K_M.gguf")
}

/// A session's raw PTY output: a title sequence, colours, a repainted status
/// line, and the things a summary must not carry (an absolute path, a key).
const SESSIONS: [(&str, &str); 3] = [
    (
        "a bug fix",
        "\x1b]0;claude\x07\x1b[1m> fix the failing date parser test\x1b[0m\r\n\
         \x1b[2K\x1b[1A\x1b[2K\x1b[38;5;208m\u{280b} Thinking\x1b[0m\r\n\
         \x1b[2K\x1b[1A\x1b[2K\x1b[38;5;208m\u{2819} Thinking\x1b[0m\r\n\
         I'll look at the test first.\r\n\
         \x1b[32m\u{25cf}\x1b[0m Read(/Users/sam/projects/calendar/src/parse/date_parser.rs)\r\n\
         The parser treats a two-digit year as a literal year, so 03/04/26 becomes the year 26.\r\n\
         \x1b[32m\u{25cf}\x1b[0m Update(/Users/sam/projects/calendar/src/parse/date_parser.rs)\r\n\
         \x1b[32m+\x1b[0m     let year = if year < 100 { 2000 + year } else { year };\r\n\
         \x1b[32m\u{25cf}\x1b[0m Bash(cargo test -p calendar date_parser)\r\n\
         running 14 tests\r\n\
         test result: ok. 14 passed; 0 failed; 0 ignored\r\n\
         The two-digit year case now parses as 2026 and all 14 date parser tests pass.\r\n",
    ),
    (
        "a deploy that printed a key",
        "\x1b]0;codex\x07\x1b[1m> deploy the staging worker\x1b[0m\r\n\
         \x1b[36m$\x1b[0m export CLOUDFLARE_API_TOKEN=cf-live-8d7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b\r\n\
         \x1b[36m$\x1b[0m wrangler deploy --env staging\r\n\
         Total Upload: 412.18 KiB / gzip: 96.02 KiB\r\n\
         Uploaded staging-worker (3.41 sec)\r\n\
         Published staging-worker (0.52 sec)\r\n\
         https://staging-worker.sam-dev-account.workers.dev\r\n\
         \x1b[31mwarning:\x1b[0m the compatibility date is more than a year old\r\n\
         The staging worker is deployed. The compatibility date in wrangler.toml should be updated.\r\n",
    ),
    // Long plain names. A summary that names them is doing its job, and must
    // not be discarded as a copy.
    (
        "a refactor that names long files and functions",
        "\x1b]0;claude\x07\x1b[1m> the session end write should clear stale fields\x1b[0m\r\n\
         On branch terminal-capture-summary-rework\r\n\
         \x1b[32m\u{25cf}\x1b[0m Read(packages/daemon/src/services/capture_service.rs)\r\n\
         build_session_end_properties only clears the summary when capture saves one.\r\n\
         \x1b[32m\u{25cf}\x1b[0m Update(packages/daemon/src/services/capture_service.rs)\r\n\
         \x1b[32m\u{25cf}\x1b[0m Bash(cargo nextest run -p nodespace-daemon capture_service)\r\n\
         \x1b[33mwarning\x1b[0m: unused variable `saves_transcript` in build_session_end_properties\r\n\
         Summary: 14 tests run: 14 passed, 0 skipped\r\n\
         build_session_end_properties now writes every field each time, and the capture_service tests pass.\r\n",
    ),
];

fn capture_of(raw: &str) -> SessionCapture {
    let mut capture = SessionCapture::new();
    capture.push(OutputChunk {
        data: raw.as_bytes().to_vec(),
        timestamp: Utc::now(),
    });
    capture
}

#[tokio::test]
#[ignore = "requires the locked native GGUF on disk"]
async fn the_native_model_summarizes_a_terminal_session_in_prose() {
    let config = ChatConfig {
        n_ctx: LIVE_N_CTX,
        ..Default::default()
    };
    let engine: Arc<dyn ChatInferenceEngine> = Arc::new(
        LlamaChatInferenceEngine::load(&model_path(), ModelFamily::Gemma4, config)
            .expect("load the native model"),
    );
    assert!(engine.runs_on_this_machine());

    for (name, raw) in SESSIONS {
        let plain_text = capture_of(raw).plain_text();
        println!("\n=== {name}: what the model is shown ===\n{plain_text}");

        let mut kept = 0;
        for rep in 0..REPS {
            let summary = generate_summary(&engine, &plain_text).await;
            println!("--- {name}, rep {rep}: {summary:?}");
            let Some(summary) = summary else { continue };
            kept += 1;

            // Enforced whatever the model wrote.
            assert!(!summary.chars().any(char::is_control), "{summary:?}");
            assert_eq!(
                copied_from_output(&summary, &plain_text),
                None,
                "{summary:?}"
            );
            assert!(!summary.contains("cf-live-"), "{summary:?}");
            assert!(!summary.contains("/Users/"), "{summary:?}");
            // And it is prose of a few sentences, not a fragment.
            assert!(summary.split_whitespace().count() >= 8, "{summary:?}");
        }
        assert!(
            kept * 2 > REPS,
            "{name}: only {kept} of {REPS} replies were usable as a summary"
        );
    }
}
