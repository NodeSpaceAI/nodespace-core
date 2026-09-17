//! Background titling for ai-chat nodes.
//!
//! A chat is created titled `"Untitled"`. Once it has accumulated a few
//! messages, the model summarises the conversation into a short title — but
//! only while it is otherwise idle, in a context of its own, and only if the
//! user has not titled the chat themselves.
//!
//! # Where the title lives
//!
//! In `node.content`. The ai-chat schema declares no `title` field
//! (`AiChatNode::into_node` sets `title: None`), and both display surfaces —
//! the viewer header and the sidebar list — already read `content`. So the
//! write here is a plain `NodeUpdate::with_content`, and the frontend picks it
//! up through the `node:updated` path it already has for exactly this
//! ("background titling updated a chat's content").
//!
//! # Never clobbering a user's title
//!
//! [`is_untitled`] is the whole guard: the titler writes only when `content`
//! is still the exact `"Untitled"` sentinel. Anything else is treated as the
//! client's own words and left alone. The guard is re-checked immediately
//! before the write, not just before generating, because generation takes
//! seconds and the user may rename the chat while it runs.
//!
//! Blank content is not a third case. `AiChatNodeBehavior::validate` rejects
//! an ai-chat node with empty or whitespace-only content, so every persisted
//! chat has a title and titling is something a client opts into by writing
//! the sentinel — not something it receives by saying nothing.
//!
//! Known consequence of using a sentinel string rather than a separate flag: a
//! user who types exactly `"Untitled"` is indistinguishable from an untitled
//! chat, and will have that title replaced once. This is the representation
//! the issue specifies.
//!
//! # Isolation from the live conversation
//!
//! Generation builds its own [`InferenceRequest`] with `tools: None` and a
//! single user message, and calls the engine directly — no session, no
//! `properties.messages` write-back, nothing appended to the conversation. It
//! reads the messages only to render a prompt string. This is the same shape
//! the ReAct loop's own history summarization uses.

use std::sync::Arc;

use nodespace_agent::agent_types::{
    ChatInferenceEngine, ChatMessage, InferenceRequest, Role, StreamingChunk,
};
use nodespace_agent::local_agent::prompt_templates::title_generation_prompt;
use nodespace_core::models::{AiChatNode, NodeUpdate};
use nodespace_core::services::{NodeService, NodeServiceError};

/// The title a new ai-chat node carries until it is titled.
///
/// Byte-identical to `UNTITLED_CHAT_TITLE` in
/// `packages/desktop-app/src/lib/utils/ai-chat-title.ts`, which is what
/// actually writes it at creation. A drift between the two disables titling
/// silently — every chat would look user-titled — so the frontend test
/// `seeds ai-chat with the exact sentinel the daemon tests for` pins the
/// string on that side.
pub const UNTITLED_CHAT_TITLE: &str = "Untitled";

/// How many messages a conversation needs before it is worth titling.
///
/// Three is two user turns plus a reply: enough for a title to describe the
/// conversation rather than just restating the opening message, while still
/// titling early enough that the sidebar is useful. Adjust here — it is the
/// single definition.
pub const TITLE_MESSAGE_THRESHOLD: usize = 3;

/// How many messages of the conversation's opening to feed the titler.
///
/// A title describes what a conversation is *about*, which is established
/// early; later turns drift into detail. Capping the input also keeps this
/// prompt small, which matters because it shares a context budget with a
/// small local model.
const TITLE_CONTEXT_MESSAGES: usize = 6;

/// Per-message character cap when rendering the conversation for the prompt.
const TITLE_CONTEXT_CHARS_PER_MESSAGE: usize = 500;

/// Longest title accepted from the model, in characters.
///
/// A small local model that ignores "at most 6 words" tends to fail loudly —
/// returning a sentence or a whole paragraph — rather than subtly. Truncating
/// at a word boundary keeps a runaway reply from becoming a sidebar-breaking
/// title.
const MAX_TITLE_CHARS: usize = 80;

/// Sampling temperature for titling. Low, but not zero: a title is a short
/// piece of prose, and greedy decoding on some local models degenerates into
/// repetition on short outputs.
const TITLE_TEMPERATURE: f32 = 0.3;

/// Token budget for the reply. Generous relative to six words so a model that
/// adds a preamble still emits the title itself before being cut off —
/// [`sanitize_title`] strips the preamble afterwards.
const TITLE_MAX_TOKENS: u32 = 48;

/// Whether `content` is a title background titling may replace.
///
/// True only for the exact `"Untitled"` sentinel; anything else is the
/// client's own title and is left alone. Blank content is not a case this has
/// to consider — `AiChatNodeBehavior::validate` rejects it outright, so a
/// titleless chat cannot be persisted in the first place.
///
/// That rejection is what makes titling an opt-in rather than a default: a
/// client asks for it by writing the sentinel, instead of getting it by
/// omitting a title.
pub fn is_untitled(content: &str) -> bool {
    content.trim() == UNTITLED_CHAT_TITLE
}

/// Whether this chat should be titled: enough conversation, and no title yet.
pub fn needs_title(chat: &AiChatNode) -> bool {
    is_untitled(&chat.content) && chat.messages.len() >= TITLE_MESSAGE_THRESHOLD
}

/// Render the opening of a conversation as the prompt's input block.
///
/// Only `user` and `assistant` messages are included: `system` messages carry
/// workspace scaffolding rather than anything the conversation is about, and
/// feeding them in produces titles about NodeSpace itself.
pub fn render_for_title(chat: &AiChatNode) -> String {
    chat.messages
        .iter()
        .filter(|m| m.role == "user" || m.role == "assistant")
        .take(TITLE_CONTEXT_MESSAGES)
        .map(|m| {
            let body: String = m
                .content
                .chars()
                .take(TITLE_CONTEXT_CHARS_PER_MESSAGE)
                .collect();
            format!("{}: {}", m.role, body.trim())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Clean up the model's reply into something usable as a title.
///
/// Small local models wrap titles in quotes, prefix them with "Title:", or
/// answer in several lines despite being told not to. Returns `None` when
/// nothing usable survives, which the caller treats as "leave the chat
/// untitled and try again after the next turn" rather than as an error.
pub fn sanitize_title(raw: &str) -> Option<String> {
    // Take the first non-empty line: a compliant reply is one line, and a
    // chatty one puts the title on the first and commentary after it.
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;

    // Strip a leading "Title:" label. Only the colon form — that is what
    // models actually emit, and splitting on a dash would eat the second half
    // of a legitimately hyphenated title.
    let line = match line.split_once(':') {
        Some((label, rest)) if label.trim().eq_ignore_ascii_case("title") => rest.trim(),
        _ => line,
    };

    // Strip symmetric wrapping quotes, which models add freely.
    let line = line
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .trim();

    // Drop trailing sentence punctuation — a title is not a sentence.
    let line = line.trim_end_matches(['.', '!', '?']).trim();

    if line.is_empty() {
        return None;
    }

    // A reply that ignored the length instruction gets truncated at a word
    // boundary rather than mid-word.
    if line.chars().count() <= MAX_TITLE_CHARS {
        return Some(line.to_string());
    }
    let truncated: String = line.chars().take(MAX_TITLE_CHARS).collect();
    let cut = match truncated.rsplit_once(char::is_whitespace) {
        Some((head, _)) if !head.trim().is_empty() => head.trim().to_string(),
        _ => truncated.trim().to_string(),
    };
    if cut.is_empty() {
        None
    } else {
        Some(cut)
    }
}

/// Generate a title for `chat` with a one-shot call that shares nothing with
/// the live conversation.
///
/// Returns `None` when the model produced nothing usable.
pub async fn generate_title(
    engine: &Arc<dyn ChatInferenceEngine>,
    chat: &AiChatNode,
) -> Option<String> {
    let conversation = render_for_title(chat);
    if conversation.trim().is_empty() {
        return None;
    }

    // Self-contained: its own messages, no tools, no session. Nothing here
    // reads or writes the chat's own history.
    let request = InferenceRequest {
        messages: vec![ChatMessage::text(
            Role::User,
            title_generation_prompt(&conversation),
        )],
        tools: None,
        temperature: Some(TITLE_TEMPERATURE),
        max_tokens: Some(TITLE_MAX_TOKENS),
    };

    let collected = Arc::new(std::sync::Mutex::new(String::new()));
    let sink = collected.clone();
    let on_chunk: Box<dyn Fn(StreamingChunk) + Send> = Box::new(move |chunk| {
        // Answer tokens only. `Reasoning` spans are the model thinking aloud,
        // which would otherwise be concatenated into the title.
        if let StreamingChunk::Token { text } = chunk {
            if let Ok(mut buf) = sink.lock() {
                buf.push_str(&text);
            }
        }
    });

    if let Err(e) = engine.generate(request, on_chunk).await {
        tracing::debug!(chat_id = %chat.id, error = %e, "ai-chat title generation failed");
        return None;
    }

    let raw = collected.lock().ok()?.clone();
    sanitize_title(&raw)
}

/// Write `title` to the chat's `content`, re-checking the untitled guard
/// against freshly-read state.
///
/// The re-read is the point: generation takes seconds, and the user may have
/// renamed the chat in the meantime. Losing the race means dropping the
/// generated title, never overwriting the user's.
pub async fn write_title_if_still_untitled(
    node_service: &Arc<NodeService>,
    node_id: &str,
    title: &str,
) -> Result<bool, NodeServiceError> {
    const MAX_ATTEMPTS: usize = 3;

    for attempt in 0..MAX_ATTEMPTS {
        let Some(node) = node_service.get_node(node_id).await? else {
            return Ok(false);
        };
        let version = node.version;

        // Re-check against what is stored right now, not what we generated from.
        if !is_untitled(&node.content) {
            tracing::debug!(
                node_id,
                "ai-chat gained a title while one was being generated; keeping the existing one"
            );
            return Ok(false);
        }

        // Setting only `content` on the `NodeUpdate` is NOT what makes this
        // safe against the turn path's concurrent message/status writes —
        // `update_node` is a full-row read-modify-write and rewrites the
        // `properties` column regardless (`node_service/crud.rs`). What makes
        // it safe is OCC: the row it writes back is a snapshot taken inside
        // the same version-checked transaction, so a concurrent append either
        // lands first and bumps `version` (turning this into the
        // `VersionConflict` the loop retries) or lands after and carries this
        // title forward. Do not "simplify" the retry loop away on the belief
        // that a content-only update touches a content-only column.
        let update = NodeUpdate::new().with_content(title.to_string());
        match node_service.update_node(node_id, version, update).await {
            Ok(_) => return Ok(true),
            Err(NodeServiceError::VersionConflict { .. }) if attempt + 1 < MAX_ATTEMPTS => {
                // The node changed under us — most likely the next turn's
                // message append. Re-read and re-check the guard.
                tracing::debug!(
                    node_id,
                    attempt,
                    "version conflict writing ai-chat title, retrying"
                );
            }
            Err(e) => return Err(e),
        }
    }

    tracing::debug!(
        node_id,
        "gave up writing ai-chat title after repeated conflicts"
    );
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use nodespace_core::models::AiChatMessage;

    fn message(role: &str, content: &str) -> AiChatMessage {
        AiChatMessage {
            role: role.to_string(),
            content: content.to_string(),
            timestamp: None,
            reasoning: None,
            completed_writes: Vec::new(),
            resolved_entities: Vec::new(),
            question: None,
            options: Vec::new(),
        }
    }

    fn chat(content: &str, messages: Vec<AiChatMessage>) -> AiChatNode {
        AiChatNode {
            id: "chat-1".to_string(),
            node_type: "ai-chat".to_string(),
            content: content.to_string(),
            version: 1,
            created_at: Utc::now(),
            modified_at: Utc::now(),
            turn_status: "idle".to_string(),
            session_status: "active".to_string(),
            provider: Some("native".to_string()),
            model: Some("test-model".to_string()),
            messages,
        }
    }

    #[test]
    fn only_the_sentinel_is_untitled() {
        assert!(is_untitled("Untitled"));
        assert!(is_untitled("  Untitled  "));
    }

    /// Blank content is not claimable. It is also not persistable —
    /// `AiChatNodeBehavior::validate` rejects it — so this pins the guard
    /// against someone reintroducing an empty-content branch here rather than
    /// describing a state the database can hold.
    #[test]
    fn blank_content_is_not_claimable() {
        assert!(!is_untitled(""));
        assert!(!is_untitled("   "));
        assert!(!is_untitled("\t\n"));
    }

    #[test]
    fn a_user_title_is_not_untitled() {
        assert!(!is_untitled("Billing architecture"));
        // Case matters: only the exact sentinel is claimable.
        assert!(!is_untitled("untitled"));
        assert!(!is_untitled("Untitled thoughts"));
    }

    #[test]
    fn needs_title_requires_both_the_threshold_and_an_untitled_chat() {
        let msgs = vec![
            message("user", "a"),
            message("assistant", "b"),
            message("user", "c"),
        ];
        assert!(needs_title(&chat(UNTITLED_CHAT_TITLE, msgs.clone())));

        // Enough messages, but the user named it.
        assert!(!needs_title(&chat("My chat", msgs)));

        // Untitled, but too early.
        assert!(!needs_title(&chat(
            UNTITLED_CHAT_TITLE,
            vec![message("user", "a")]
        )));
    }

    #[test]
    fn render_skips_system_messages_and_caps_length() {
        let rendered = render_for_title(&chat(
            UNTITLED_CHAT_TITLE,
            vec![
                message("system", "workspace scaffolding"),
                message("user", "how do I reset my password"),
                message("assistant", "Open settings."),
            ],
        ));
        assert!(!rendered.contains("scaffolding"));
        assert!(rendered.contains("reset my password"));
        assert!(rendered.starts_with("user:"));

        let long = "x".repeat(TITLE_CONTEXT_CHARS_PER_MESSAGE * 3);
        let rendered = render_for_title(&chat(UNTITLED_CHAT_TITLE, vec![message("user", &long)]));
        assert!(rendered.chars().count() <= TITLE_CONTEXT_CHARS_PER_MESSAGE + "user: ".len());
    }

    #[test]
    fn sanitize_strips_model_decoration() {
        assert_eq!(
            sanitize_title("Billing setup").as_deref(),
            Some("Billing setup")
        );
        assert_eq!(
            sanitize_title("\"Billing setup\"").as_deref(),
            Some("Billing setup")
        );
        assert_eq!(
            sanitize_title("Title: Billing setup").as_deref(),
            Some("Billing setup")
        );
        assert_eq!(
            sanitize_title("  Billing setup.  ").as_deref(),
            Some("Billing setup")
        );
        assert_eq!(
            sanitize_title("Billing setup\nLet me know if you want another!").as_deref(),
            Some("Billing setup")
        );
    }

    #[test]
    fn sanitize_rejects_empty_replies() {
        assert!(sanitize_title("").is_none());
        assert!(sanitize_title("   \n  ").is_none());
        assert!(sanitize_title("\"\"").is_none());
        assert!(sanitize_title("...").is_none());
    }

    #[test]
    fn sanitize_truncates_a_runaway_reply_at_a_word_boundary() {
        let rambling = "This is a very long title that the model produced despite being \
                        told to use at most six words and it just keeps going";
        let title = sanitize_title(rambling).expect("should salvage a title");
        assert!(title.chars().count() <= MAX_TITLE_CHARS);
        // Truncated at a boundary, so the last word is not sliced in half.
        assert!(!title.ends_with(' '));
        assert!(rambling.starts_with(&title));
    }
}
