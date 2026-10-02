//! The prose summary of a finished terminal session, derived on this machine.
//!
//! A terminal chat's `summary` is the one part of what a session printed that
//! is not `local_only` (ADR-061 §7). So it is not a slice of the session's
//! output: it is prose a model writes about that output, here, from the
//! output with its escape sequences already removed
//! ([`SessionCapture::plain_text`](nodespace_agent::pty::SessionCapture::plain_text)).
//!
//! # What is checked, and what is not
//!
//! A model asked to describe output can repeat it instead. Two things are
//! enforced on its reply whatever it wrote: it holds no escape sequence or
//! control character ([`sanitize_summary`]), and it is not a copy of the
//! output ([`copied_from_output`]). A copy is a reply that:
//!
//! - is itself a passage of the output;
//! - repeats eight words running of it; or
//! - shares with it a long unbroken run of characters shaped like a path, a
//!   URL, an assignment or a key (it holds `/`, `\`, `:`, `=` or `@`, or is
//!   heavy with digits).
//!
//! A copy is discarded, and the chat has no summary. A long plain name (a
//! file, a function, a branch) is not a copy: naming what was worked on is
//! what a summary is for.
//!
//! That is a check for copying, not for secrets. A short secret, a key made
//! only of letters, or a bare host name that the model restates is not
//! caught, and nothing here detects one: scrubbing content is a separate, best-effort problem
//! (ADR-061 §7), and this module does not claim to solve it.
//!
//! # Only a model on this machine
//!
//! Terminal output can hold secrets, tokens and absolute paths, and cannot be
//! scrubbed. Deriving the summary must not be the thing that sends it
//! somewhere, so [`LocalModelSummarizer`] runs only on a model loaded on this
//! machine. With no model loaded, or with an OpenAI-compatible endpoint
//! loaded, there is no summarizer and the chat keeps no summary: ADR-061 §7's
//! metadata-only fallback.
//!
//! # Isolation from live conversations
//!
//! Like background titling (`ai_chat_title`), generation is a one-shot request
//! with no tools and no session, made once the chat model has been idle for a
//! while, so it does not take the model from a conversation in progress.

use std::sync::Arc;

use async_trait::async_trait;
use nodespace_agent::agent_types::{
    ChatInferenceEngine, ChatMessage, InferenceRequest, Role, StreamingChunk,
};
use nodespace_agent::local_agent::prompt_templates::session_summary_prompt;
use nodespace_agent::pty::plain_text::strip_terminal_sequences;

use crate::services::capture_service::SessionSummarizer;
use crate::services::local_agent_service::SharedLocalAgent;

/// How much of the start of a session's output the summarizer is shown. The
/// start is where the session's task is stated.
const SUMMARY_INPUT_HEAD_CHARS: usize = 1500;

/// How much of the end of a session's output the summarizer is shown. The end
/// is where the outcome is, so it gets the larger share. Together with the
/// head this keeps the prompt small enough for a small local model's context.
const SUMMARY_INPUT_TAIL_CHARS: usize = 4500;

/// Stands in for the output left out between the head and the tail.
const SUMMARY_INPUT_ELISION: &str = "\n[…]\n";

/// Longest summary accepted from the model, in characters. A reply that
/// ignores "two or three sentences" is cut at a sentence end.
const MAX_SUMMARY_CHARS: usize = 600;

/// How many words running the reply may not share with the output. Shorter
/// phrases ("all 42 tests pass") are ones a faithful summary may well use.
const COPIED_WORDS: usize = 8;

/// The length of an unbroken run of characters, with no space in it, that the
/// reply may not share with the output when the run is shaped like a path, a
/// URL, an assignment or a key (see [`is_machine_text`]). Ordinary words are
/// shorter.
const COPIED_RUN_CHARS: usize = 20;

/// How many digits make a run of [`COPIED_RUN_CHARS`] characters read as a
/// key, a hash or a token rather than a name. A branch or a versioned file
/// name carries a few; a key carries many.
const KEY_LIKE_DIGITS: usize = 8;

/// Sampling temperature. Low, but not zero, for the same reason as titling:
/// greedy decoding on some local models degenerates on short prose.
const SUMMARY_TEMPERATURE: f32 = 0.3;

/// Token budget for the reply: room for three sentences and a preamble that
/// [`sanitize_summary`] then strips.
const SUMMARY_MAX_TOKENS: u32 = 200;

/// How long the chat model must stay idle before a summary is generated.
const SUMMARY_QUIET_PERIOD: std::time::Duration = std::time::Duration::from_secs(5);

/// The part of a session's plain-text output the summarizer is shown: all of
/// it when short, otherwise its start and its end.
pub fn render_for_summary(plain_text: &str) -> String {
    let total = plain_text.chars().count();
    if total <= SUMMARY_INPUT_HEAD_CHARS + SUMMARY_INPUT_TAIL_CHARS {
        return plain_text.to_string();
    }
    let head: String = plain_text.chars().take(SUMMARY_INPUT_HEAD_CHARS).collect();
    let tail: String = plain_text
        .chars()
        .skip(total - SUMMARY_INPUT_TAIL_CHARS)
        .collect();
    format!("{head}{SUMMARY_INPUT_ELISION}{tail}")
}

/// Clean up the model's reply into one paragraph of prose.
///
/// The reply goes through the same stripping as terminal output, so whatever
/// the model echoed, the summary holds no escape sequence or control
/// character. Returns `None` when nothing readable survives, which leaves the
/// chat without a summary.
pub fn sanitize_summary(raw: &str) -> Option<String> {
    let plain = strip_terminal_sequences(raw);
    // One paragraph: a small model that answers in lines or a list still
    // yields a single run of prose.
    let paragraph = plain.split_whitespace().collect::<Vec<_>>().join(" ");

    // Strip a leading "Summary:" label and wrapping quotes, which models add
    // freely.
    let paragraph = match paragraph.split_once(':') {
        Some((label, rest)) if label.trim().eq_ignore_ascii_case("summary") => rest.trim(),
        _ => paragraph.as_str(),
    };
    let paragraph = paragraph
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .trim();

    if !paragraph.chars().any(char::is_alphanumeric) {
        return None;
    }
    if paragraph.chars().count() <= MAX_SUMMARY_CHARS {
        return Some(paragraph.to_string());
    }

    // A runaway reply is cut at the last sentence end that fits, or failing
    // that at a word boundary.
    let truncated: String = paragraph.chars().take(MAX_SUMMARY_CHARS).collect();
    // A sentence ends at punctuation followed by a space: the `.` inside
    // `parser.rs` or `v1.2` is not one.
    let sentence_end = [". ", "! ", "? "]
        .iter()
        .filter_map(|end| truncated.rfind(end))
        .max();
    if let Some(end) = sentence_end {
        return Some(truncated[..=end].to_string());
    }
    let cut = match truncated.rsplit_once(char::is_whitespace) {
        Some((head, _)) if !head.trim().is_empty() => head.trim(),
        _ => truncated.trim(),
    };
    Some(format!("{cut}…"))
}

/// How a reply repeats the session's output instead of describing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopiedOutput {
    /// The whole reply is a passage of the output.
    Passage,
    /// The reply repeats [`COPIED_WORDS`] words running of the output.
    Words,
    /// The reply shares with the output a run of [`COPIED_RUN_CHARS`]
    /// characters shaped like a path, a URL, an assignment or a key.
    MachineText,
}

/// How `summary` copies the session's output, if it does.
///
/// Words are compared with whitespace collapsed, as [`sanitize_summary`]
/// leaves it, so a copy that runs across a line break of the output is still
/// one. A shared run need not be a whole word of either side: the value
/// copied out of `KEY=value` counts.
pub fn copied_from_output(summary: &str, plain_text: &str) -> Option<CopiedOutput> {
    let output = plain_text.split_whitespace().collect::<Vec<_>>().join(" ");
    let words: Vec<&str> = summary.split_whitespace().collect();

    if !words.is_empty() && output.contains(&words.join(" ")) {
        return Some(CopiedOutput::Passage);
    }
    // The punctuation a sentence puts around a quoted run is not part of it.
    if words
        .windows(COPIED_WORDS)
        .any(|run| output.contains(run.join(" ").trim_matches(|c: char| !c.is_alphanumeric())))
    {
        return Some(CopiedOutput::Words);
    }
    let shares_machine_text = words.iter().any(|word| {
        let chars: Vec<char> = word.chars().collect();
        chars
            .windows(COPIED_RUN_CHARS)
            .any(|run| is_machine_text(run) && output.contains(&run.iter().collect::<String>()))
    });
    shares_machine_text.then_some(CopiedOutput::MachineText)
}

/// Whether an unbroken run of characters is shaped like a path, a URL, an
/// assignment or a key, as opposed to a long name such as
/// `build_session_end_properties`.
fn is_machine_text(run: &[char]) -> bool {
    run.iter()
        .any(|c| matches!(c, '/' | '\\' | ':' | '=' | '@'))
        || run.iter().filter(|c| c.is_ascii_digit()).count() >= KEY_LIKE_DIGITS
}

/// Summarize a session's plain-text output with a one-shot request to
/// `engine`.
///
/// Returns `None` when there is nothing to summarize, when generation fails,
/// when the model produced nothing usable, or when its reply copies the
/// output.
pub async fn generate_summary(
    engine: &Arc<dyn ChatInferenceEngine>,
    plain_text: &str,
) -> Option<String> {
    if !plain_text.chars().any(char::is_alphanumeric) {
        return None;
    }

    let request = InferenceRequest {
        messages: vec![ChatMessage::text(
            Role::User,
            session_summary_prompt(&render_for_summary(plain_text)),
        )],
        tools: None,
        temperature: Some(SUMMARY_TEMPERATURE),
        max_tokens: Some(SUMMARY_MAX_TOKENS),
    };

    let collected = Arc::new(std::sync::Mutex::new(String::new()));
    let sink = collected.clone();
    let on_chunk: Box<dyn Fn(StreamingChunk) + Send> = Box::new(move |chunk| {
        // Answer tokens only: `Reasoning` spans are the model thinking aloud.
        if let StreamingChunk::Token { text } = chunk {
            if let Ok(mut buf) = sink.lock() {
                buf.push_str(&text);
            }
        }
    });

    if let Err(e) = engine.generate(request, on_chunk).await {
        tracing::debug!(error = %e, "terminal session summary generation failed");
        return None;
    }

    let raw = collected.lock().ok()?.clone();
    let summary = sanitize_summary(&raw)?;
    if let Some(copied) = copied_from_output(&summary, plain_text) {
        // Logged without the reply: it is the output it copied.
        tracing::info!(
            ?copied,
            "terminal session summary discarded: it repeated the session's output"
        );
        return None;
    }
    Some(summary)
}

/// Summarizes with the chat model loaded on this machine, when there is one.
pub struct LocalModelSummarizer {
    shared: Arc<SharedLocalAgent>,
}

impl LocalModelSummarizer {
    pub fn new(shared: Arc<SharedLocalAgent>) -> Self {
        Self { shared }
    }
}

#[async_trait]
impl SessionSummarizer for LocalModelSummarizer {
    async fn summarize(&self, plain_text: &str) -> Option<String> {
        // Checked before waiting, so a machine with no local model does not
        // hold the session's task open for the quiet period.
        self.shared.local_engine().await?;
        self.shared
            .idle_gate()
            .wait_for_idle_stable(SUMMARY_QUIET_PERIOD)
            .await;
        // Read again: the model may have been unloaded or swapped for a
        // remote one during the wait.
        let engine = self.shared.local_engine().await?;
        generate_summary(&engine, plain_text).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_agent::agent_types::{ChatModelSpec, InferenceError, InferenceUsage};
    use std::sync::Mutex;

    /// An engine that replies with a fixed text and records the prompt it was
    /// given.
    struct ScriptedEngine {
        reply: Result<&'static str, ()>,
        prompts: Mutex<Vec<String>>,
    }

    impl ScriptedEngine {
        fn replying(reply: &'static str) -> Arc<Self> {
            Arc::new(Self {
                reply: Ok(reply),
                prompts: Mutex::new(Vec::new()),
            })
        }

        fn failing() -> Arc<Self> {
            Arc::new(Self {
                reply: Err(()),
                prompts: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl ChatInferenceEngine for ScriptedEngine {
        async fn generate(
            &self,
            request: InferenceRequest,
            on_chunk: Box<dyn Fn(StreamingChunk) + Send>,
        ) -> Result<InferenceUsage, InferenceError> {
            assert!(
                request.tools.is_none(),
                "a summary request carries no tools"
            );
            let prompt = request
                .messages
                .iter()
                .map(|m| m.content.clone())
                .collect::<Vec<_>>()
                .join("\n");
            self.prompts.lock().unwrap().push(prompt);
            let reply = self.reply.map_err(|()| InferenceError::NoModelLoaded)?;
            on_chunk(StreamingChunk::Reasoning {
                text: "thinking about the session".to_string(),
            });
            on_chunk(StreamingChunk::Token {
                text: reply.to_string(),
            });
            Ok(InferenceUsage::default())
        }

        async fn model_info(&self) -> Result<Option<ChatModelSpec>, InferenceError> {
            Ok(None)
        }

        async fn token_count(&self, text: &str) -> Result<u32, InferenceError> {
            Ok(text.len() as u32)
        }
    }

    fn engine(scripted: &Arc<ScriptedEngine>) -> Arc<dyn ChatInferenceEngine> {
        scripted.clone()
    }

    #[tokio::test]
    async fn the_summary_is_the_models_answer_not_its_reasoning() {
        let scripted = ScriptedEngine::replying("Fixed the parser and added tests.");
        let summary =
            generate_summary(&engine(&scripted), "Edit parser.rs\nAll 42 tests pass").await;
        assert_eq!(
            summary.as_deref(),
            Some("Fixed the parser and added tests.")
        );

        let prompts = scripted.prompts.lock().unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("All 42 tests pass"));
    }

    #[tokio::test]
    async fn a_session_that_printed_nothing_readable_is_not_summarized() {
        let scripted = ScriptedEngine::replying("Nothing happened.");
        for output in ["", "  \n ", "────"] {
            assert_eq!(generate_summary(&engine(&scripted), output).await, None);
        }
        assert!(
            scripted.prompts.lock().unwrap().is_empty(),
            "the model is not asked"
        );
    }

    #[tokio::test]
    async fn a_failed_generation_leaves_no_summary() {
        let scripted = ScriptedEngine::failing();
        assert_eq!(
            generate_summary(&engine(&scripted), "some output").await,
            None
        );
    }

    #[tokio::test]
    async fn an_unusable_reply_leaves_no_summary() {
        let scripted = ScriptedEngine::replying("  \n...\n");
        assert_eq!(
            generate_summary(&engine(&scripted), "some output").await,
            None
        );
    }

    /// The whole path a session's output takes to its summary: the model is
    /// shown no escape sequence, and whatever it writes back, the summary
    /// holds none.
    #[tokio::test]
    async fn output_with_escape_sequences_yields_a_summary_with_none() {
        use nodespace_agent::pty::{OutputChunk, SessionCapture};

        let mut capture = SessionCapture::new();
        capture.push(OutputChunk {
            data: b"\x1b]0;claude\x07\x1b[1;32mEdited parser.rs\x1b[0m\r\n\x1b[2KAll 42 tests pass\r\n"
                .to_vec(),
            timestamp: chrono::Utc::now(),
        });
        let scripted =
            ScriptedEngine::replying("\x1b[1mFixed\x1b[0m the parser;\x07 its tests pass.\r\n");

        let summary = generate_summary(&engine(&scripted), &capture.plain_text())
            .await
            .expect("a summary");

        assert_eq!(summary, "Fixed the parser; its tests pass.");
        assert!(!summary.chars().any(char::is_control));
        let prompts = scripted.prompts.lock().unwrap();
        assert!(prompts[0].contains("Edited parser.rs\nAll 42 tests pass"));
        assert!(!prompts[0].chars().any(|c| c.is_control() && c != '\n'));
    }

    /// A model that answers with the output it was shown, rather than with
    /// prose about it, leaves the chat without a summary.
    #[tokio::test]
    async fn a_reply_that_repeats_the_output_is_discarded() {
        let output = "export API_KEY=sk-live-9f8e7d6c5b4a39281706f5e4\n\
                      Running the parser test suite against the new grammar\n\
                      All 42 tests pass";

        for echo in [
            // The whole output.
            "export API_KEY=sk-live-9f8e7d6c5b4a39281706f5e4 Running the parser test suite against the new grammar All 42 tests pass",
            // One line of it, inside prose.
            "The agent said: Running the parser test suite against the new grammar.",
            // Part of a line, running on into the next.
            "It was parser test suite against the new grammar All 42 tests, roughly.",
            // The value of the key alone, lifted out of its line.
            "The agent set the key (sk-live-9f8e7d6c5b4a39281706f5e4) and ran the tests.",
        ] {
            let scripted = ScriptedEngine::replying(echo);
            assert_eq!(
                generate_summary(&engine(&scripted), output).await,
                None,
                "{echo}"
            );
        }

        // A short session echoed back whole: too short for a run of words or
        // a long token, and still nothing but the output.
        let short = "Edit parser.rs\nAll 42 tests pass\nexport TOKEN=abc123def456";
        let scripted = ScriptedEngine::replying("Edit parser.rs All 42 tests pass");
        assert_eq!(generate_summary(&engine(&scripted), short).await, None);

        // Prose about the output, short phrases of it included, is kept.
        let scripted =
            ScriptedEngine::replying("Set an API key and ran the parser tests; all 42 tests pass.");
        assert_eq!(
            generate_summary(&engine(&scripted), output)
                .await
                .as_deref(),
            Some("Set an API key and ran the parser tests; all 42 tests pass.")
        );
    }

    #[test]
    fn a_copy_is_a_passage_a_run_of_words_or_machine_text() {
        let output = "  cd   /Users/sam/projects/nodespace/core  \nshort line\n\
                      Refactored the tokenizer so that it can handle\n nested brackets";

        // The reply is a passage of the output, however short.
        assert_eq!(
            copied_from_output("short line", output),
            Some(CopiedOutput::Passage)
        );
        // Eight words running, whitespace collapsed, across a line break.
        assert_eq!(
            copied_from_output(
                "It printed: tokenizer so that it can handle nested brackets, then stopped.",
                output
            ),
            Some(CopiedOutput::Words)
        );
        // A path, whatever punctuation the summary wraps it in.
        assert_eq!(
            copied_from_output("Worked in (/Users/sam/projects/nodespace/core).", output),
            Some(CopiedOutput::MachineText)
        );

        // Seven words running, and phrases of the output, are not copies.
        assert_eq!(
            copied_from_output(
                "It refactored the tokenizer so that it can cope with nested brackets.",
                output
            ),
            None
        );
        assert_eq!(
            copied_from_output("A short line about the tokenizer.", output),
            None
        );
        assert_eq!(copied_from_output("Anything at all.", ""), None);
    }

    /// Naming what was worked on is what a summary is for: a long file,
    /// function or branch name taken from the output is not a copy. A path,
    /// a URL, an assignment and a key are.
    #[test]
    fn a_long_name_is_not_machine_text_and_a_path_or_a_key_is() {
        let output = "Update(packages/daemon/src/services/local_agent_service.rs)\n\
                      warning: unused variable in build_session_end_properties\n\
                      On branch issue-3454-pty-capture-summary\n\
                      Published https://staging-worker.sam-dev-account.workers.dev\n\
                      export AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMIK7MDENGbPxRfiCY\n\
                      commit 9f8e7d6c5b4a39281706f5e4d3c2b1a098765432";

        for kept in [
            "Edited local_agent_service.rs and ran the tests.",
            "Fixed a warning in build_session_end_properties.",
            "Worked on branch issue-3454-pty-capture-summary.",
        ] {
            assert_eq!(copied_from_output(kept, output), None, "{kept}");
        }

        for copied in [
            "Edited packages/daemon/src/services/local_agent_service.rs.",
            "Deployed to https://staging-worker.sam-dev-account.workers.dev.",
            "Set AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMIK7MDENGbPxRfiCY first.",
            "Made commit 9f8e7d6c5b4a39281706f5e4d3c2b1a098765432.",
        ] {
            assert_eq!(
                copied_from_output(copied, output),
                Some(CopiedOutput::MachineText),
                "{copied}"
            );
        }

        // What the check does not claim to catch: a key of letters alone
        // lifted out of its assignment, and a bare host name. Neither can be
        // told from a long name.
        for missed in [
            "The key is wJalrXUtnFEMIK7MDENGbPxRfiCY.",
            "Published to staging-worker.sam-dev-account.workers.dev for testing.",
        ] {
            assert_eq!(copied_from_output(missed, output), None, "{missed}");
        }
    }

    #[test]
    fn a_long_session_is_shown_by_its_start_and_its_end() {
        let short = "one\ntwo";
        assert_eq!(render_for_summary(short), short);

        let long = format!(
            "START{}MIDDLE{}END",
            "a".repeat(SUMMARY_INPUT_HEAD_CHARS),
            "b".repeat(SUMMARY_INPUT_TAIL_CHARS)
        );
        let rendered = render_for_summary(&long);
        assert!(rendered.starts_with("START"));
        assert!(rendered.ends_with("END"));
        assert!(!rendered.contains("MIDDLE"));
        assert_eq!(
            rendered.chars().count(),
            SUMMARY_INPUT_HEAD_CHARS
                + SUMMARY_INPUT_TAIL_CHARS
                + SUMMARY_INPUT_ELISION.chars().count()
        );
    }

    #[test]
    fn sanitize_strips_model_decoration() {
        assert_eq!(
            sanitize_summary("Summary: Fixed the parser.").as_deref(),
            Some("Fixed the parser.")
        );
        assert_eq!(
            sanitize_summary("\"Fixed the parser.\"").as_deref(),
            Some("Fixed the parser.")
        );
        assert_eq!(
            sanitize_summary("Fixed the parser.\n\nAdded tests:\n- one\n- two").as_deref(),
            Some("Fixed the parser. Added tests: - one - two")
        );
    }

    /// Whatever the model echoes from the output it was shown, the summary
    /// holds no escape sequence or control character.
    #[test]
    fn sanitize_removes_escape_sequences_and_control_characters() {
        let summary =
            sanitize_summary("\x1b[1mFixed\x1b[0m the parser.\x07\r\n\x1b]0;title\x07Done.\t")
                .unwrap();
        assert_eq!(summary, "Fixed the parser. Done.");
        assert!(!summary.chars().any(char::is_control));
    }

    #[test]
    fn sanitize_cuts_a_runaway_reply_at_a_sentence_end() {
        let sentence = "The session refactored the parser and fixed its tests. ";
        let rambling = sentence.repeat(30);
        let summary = sanitize_summary(&rambling).unwrap();
        assert!(summary.chars().count() <= MAX_SUMMARY_CHARS);
        assert!(summary.ends_with("tests."), "{summary}");

        // The `.` in a file name or a version is not a sentence end.
        let dotted = format!(
            "The session fixed the parser. {}",
            "It then touched parser.rs and v1.2 again ".repeat(30)
        );
        assert_eq!(
            sanitize_summary(&dotted).as_deref(),
            Some("The session fixed the parser.")
        );

        let unbroken = "word ".repeat(300);
        let summary = sanitize_summary(&unbroken).unwrap();
        assert!(summary.chars().count() <= MAX_SUMMARY_CHARS + 1);
        assert!(summary.ends_with("word…"), "{summary}");
    }

    #[test]
    fn sanitize_rejects_a_reply_with_nothing_to_read() {
        assert_eq!(sanitize_summary(""), None);
        assert_eq!(sanitize_summary("\x1b[0m\n  \n"), None);
        assert_eq!(sanitize_summary("\"\""), None);
    }
}
