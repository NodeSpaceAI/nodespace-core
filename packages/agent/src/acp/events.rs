//! The typed events a session produces, and how the agent's messages are turned
//! into them. Only what the client needs is read: tool input and output, file
//! diffs and locations are never copied out of a message.

use std::sync::Arc;

use serde_json::Value;
use zeroize::Zeroizing;

use crate::local_agent::redaction::{cut, sanitize};

const MAX_ID: usize = 256;
const MAX_TITLE: usize = 1024;
const MAX_NAME: usize = 128;
/// Longest text of one streamed chunk; more is cut.
pub(super) const MAX_CHUNK: usize = 256 * 1024;
const MAX_PLAN_ENTRIES: usize = 64;
const MAX_PLAN_ENTRY: usize = 500;
const MAX_COMMANDS: usize = 64;
const MAX_OPTIONS: usize = 16;

/// What the agent said about itself to end a prompt turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    /// A reason this client does not know, cut and cleaned.
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

impl ToolKind {
    fn parse(value: &str) -> Self {
        match value {
            "read" => Self::Read,
            "edit" => Self::Edit,
            "delete" => Self::Delete,
            "move" => Self::Move,
            "search" => Self::Search,
            "execute" => Self::Execute,
            "think" => Self::Think,
            "fetch" => Self::Fetch,
            "switch_mode" => Self::SwitchMode,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl ToolStatus {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// One streamed update of a session.
#[derive(Debug, Clone, PartialEq)]
pub enum AcpUpdate {
    AgentText {
        text: String,
        message_id: Option<String>,
    },
    AgentThought {
        text: String,
        message_id: Option<String>,
    },
    ToolCall {
        tool_call_id: String,
        title: String,
        kind: ToolKind,
        status: Option<ToolStatus>,
    },
    ToolCallUpdate {
        tool_call_id: String,
        title: Option<String>,
        kind: Option<ToolKind>,
        status: Option<ToolStatus>,
    },
    /// The whole plan, rendered as a markdown checklist (an update replaces the
    /// previous plan).
    Plan {
        markdown: String,
    },
    /// The names of the slash commands the agent offers (kept in memory only).
    Commands {
        names: Vec<String>,
    },
    Mode {
        mode_id: String,
    },
    SessionTitle {
        title: Option<String>,
    },
    Usage {
        used: u64,
        size: u64,
    },
}

/// What a permission choice does. Only the once-only kinds are ever shown or
/// accepted: an "always" choice makes the agent write standing rules into the
/// user's settings, which this client never asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOptionKind {
    AllowOnce,
    RejectOnce,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
}

/// A permission request the agent is blocked on. Answer it with
/// `AcpSession::answer_permission`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRequest {
    /// This client's key for the request; not the agent's id.
    pub request: u64,
    pub tool_call_id: String,
    pub title: String,
    pub kind: ToolKind,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionResolution {
    Answered,
    /// Answered as cancelled: the turn was cancelled.
    Cancelled,
    TimedOut,
    /// The turn ended (or the session did) while it waited.
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndReason {
    /// `close` was called or the handle was dropped.
    Closed,
    /// The agent exited or closed its output.
    AgentExited,
    /// A limit or the protocol was broken; the text says which.
    Fault(String),
}

/// What a session reports, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum AcpEvent {
    Update(AcpUpdate),
    PermissionRequest(PermissionRequest),
    PermissionResolved {
        request: u64,
        resolution: PermissionResolution,
    },
    TurnEnded {
        stop_reason: StopReason,
    },
    /// The prompt was answered with an error (the session stays up).
    TurnFailed {
        code: i64,
        message: String,
        auth_required: bool,
    },
    /// Last event: the process has been stopped.
    SessionEnded(EndReason),
}

/// The credentials the agent was given, to be removed from everything it says.
pub(super) type Needles = Arc<Vec<Zeroizing<String>>>;

/// `text` with every credential removed, whole, and only then cut: a credential
/// that straddles the cut is removed before it can be halved.
pub(super) fn clean(needles: &Needles, text: &str, max: usize) -> String {
    let mut out = redact(needles, text);
    cut(&mut out, max);
    out
}

pub(super) fn redact(needles: &Needles, text: &str) -> String {
    if needles.is_empty() {
        text.to_string()
    } else {
        sanitize(text.as_bytes(), needles, false)
    }
}

/// A string the agent sent, decoded (so JSON escapes are resolved), cleaned of
/// credentials, then cut. Every string that is surfaced goes through here.
pub(super) fn bounded(value: Option<&Value>, max: usize, needles: &Needles) -> Option<String> {
    Some(clean(needles, value?.as_str()?, max))
}

fn id_of(value: Option<&Value>, needles: &Needles) -> Option<String> {
    let text = redact(needles, value?.as_str()?);
    (!text.is_empty() && text.len() <= MAX_ID && !text.chars().any(char::is_control))
        .then_some(text)
}

pub(super) fn parse_stop_reason(value: &str) -> StopReason {
    match value {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "max_turn_requests" => StopReason::MaxTurnRequests,
        "refusal" => StopReason::Refusal,
        "cancelled" => StopReason::Cancelled,
        other => {
            let mut text = other.to_string();
            cut(&mut text, MAX_NAME);
            StopReason::Other(text)
        }
    }
}

fn chunk_text(update: &Value, needles: &Needles) -> Option<String> {
    let content = update.get("content")?;
    if content.get("type").and_then(Value::as_str) != Some("text") {
        return None;
    }
    bounded(content.get("text"), MAX_CHUNK, needles)
}

fn plan_markdown(update: &Value, needles: &Needles) -> Option<String> {
    let entries = update.get("entries")?.as_array()?;
    let mut lines = Vec::new();
    for entry in entries.iter().take(MAX_PLAN_ENTRIES) {
        let content = bounded(entry.get("content"), MAX_PLAN_ENTRY, needles)?;
        let mark = match entry.get("status").and_then(Value::as_str) {
            Some("completed") => "x",
            Some("in_progress") => "~",
            _ => " ",
        };
        lines.push(format!("- [{mark}] {}", content.replace(['\n', '\r'], " ")));
    }
    Some(lines.join("\n"))
}

/// The update inside a `session/update` notification's params, or `None` when
/// it is for another session, is malformed, or is of a kind that is not kept
/// (the echo of the user's own message among them).
pub(super) fn parse_update(
    params: &Value,
    session_id: &str,
    needles: &Needles,
) -> Option<AcpUpdate> {
    if params.get("sessionId").and_then(Value::as_str) != Some(session_id) {
        return None;
    }
    let update = params.get("update")?;
    let kind = update.get("sessionUpdate")?.as_str()?;
    match kind {
        "agent_message_chunk" => Some(AcpUpdate::AgentText {
            text: chunk_text(update, needles)?,
            message_id: id_of(update.get("messageId"), needles),
        }),
        "agent_thought_chunk" => Some(AcpUpdate::AgentThought {
            text: chunk_text(update, needles)?,
            message_id: id_of(update.get("messageId"), needles),
        }),
        "tool_call" => Some(AcpUpdate::ToolCall {
            tool_call_id: id_of(update.get("toolCallId"), needles)?,
            title: bounded(update.get("title"), MAX_TITLE, needles).unwrap_or_default(),
            kind: ToolKind::parse(update.get("kind").and_then(Value::as_str).unwrap_or("")),
            status: update
                .get("status")
                .and_then(Value::as_str)
                .and_then(ToolStatus::parse),
        }),
        "tool_call_update" => Some(AcpUpdate::ToolCallUpdate {
            tool_call_id: id_of(update.get("toolCallId"), needles)?,
            title: bounded(update.get("title"), MAX_TITLE, needles),
            kind: update
                .get("kind")
                .and_then(Value::as_str)
                .map(ToolKind::parse),
            status: update
                .get("status")
                .and_then(Value::as_str)
                .and_then(ToolStatus::parse),
        }),
        "plan" => Some(AcpUpdate::Plan {
            markdown: plan_markdown(update, needles)?,
        }),
        "available_commands_update" => {
            let names = update
                .get("availableCommands")?
                .as_array()?
                .iter()
                .take(MAX_COMMANDS)
                .filter_map(|c| bounded(c.get("name"), MAX_NAME, needles))
                .collect();
            Some(AcpUpdate::Commands { names })
        }
        "current_mode_update" => Some(AcpUpdate::Mode {
            mode_id: id_of(update.get("currentModeId"), needles)?,
        }),
        "session_info_update" => Some(AcpUpdate::SessionTitle {
            title: bounded(update.get("title"), MAX_TITLE, needles),
        }),
        "usage_update" => Some(AcpUpdate::Usage {
            used: update.get("used")?.as_u64()?,
            size: update.get("size")?.as_u64()?,
        }),
        // user_message_chunk (the client wrote that message itself), config
        // options and anything newer: not kept.
        _ => None,
    }
}

/// The parts of a `session/request_permission` request this client reads, or
/// `None` when it is for another session or has no choice it may offer.
pub(super) fn parse_permission(
    params: &Value,
    session_id: &str,
    needles: &Needles,
) -> Result<(String, String, ToolKind, Vec<PermissionOption>), &'static str> {
    if params.get("sessionId").and_then(Value::as_str) != Some(session_id) {
        return Err("the request is for another session");
    }
    let tool = params
        .get("toolCall")
        .ok_or("the request names no tool call")?;
    let tool_call_id =
        id_of(tool.get("toolCallId"), needles).ok_or("the tool call has no usable id")?;
    let title = bounded(tool.get("title"), MAX_TITLE, needles).unwrap_or_default();
    let kind = ToolKind::parse(tool.get("kind").and_then(Value::as_str).unwrap_or(""));
    let offered = params
        .get("options")
        .and_then(Value::as_array)
        .ok_or("the request has no options")?;
    let mut options = Vec::new();
    for option in offered.iter().take(MAX_OPTIONS) {
        let kind = match option.get("kind").and_then(Value::as_str) {
            Some("allow_once") => PermissionOptionKind::AllowOnce,
            Some("reject_once") => PermissionOptionKind::RejectOnce,
            // allow_always, reject_always and anything else is not offered.
            _ => continue,
        };
        let (Some(option_id), Some(name)) = (
            id_of(option.get("optionId"), needles),
            bounded(option.get("name"), MAX_NAME, needles),
        ) else {
            continue;
        };
        options.push(PermissionOption {
            option_id,
            name,
            kind,
        });
    }
    Ok((tool_call_id, title, kind, options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn none() -> Needles {
        Arc::new(Vec::new())
    }

    fn upd(update: Value) -> Value {
        json!({"sessionId": "s1", "update": update})
    }

    #[test]
    fn parses_text_thought_tool_plan_usage() {
        let t = parse_update(
            &upd(
                json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"},"messageId":"m1"}),
            ),
            "s1",
            &none(),
        );
        assert_eq!(
            t,
            Some(AcpUpdate::AgentText {
                text: "hi".into(),
                message_id: Some("m1".into())
            })
        );
        let p = parse_update(
            &upd(json!({"sessionUpdate":"plan","entries":[
                {"content":"a\nb","priority":"high","status":"completed"},
                {"content":"c","priority":"low","status":"pending"}]})),
            "s1",
            &none(),
        );
        assert_eq!(
            p,
            Some(AcpUpdate::Plan {
                markdown: "- [x] a b\n- [ ] c".into()
            })
        );
        assert!(matches!(
            parse_update(
                &upd(
                    json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"Run","kind":"execute","status":"pending","rawInput":{"secret":"x"}})
                ),
                "s1",
                &none()
            ),
            Some(AcpUpdate::ToolCall {
                kind: ToolKind::Execute,
                status: Some(ToolStatus::Pending),
                ..
            })
        ));
        assert_eq!(
            parse_update(
                &upd(json!({"sessionUpdate":"usage_update","used":5,"size":9})),
                "s1",
                &none()
            ),
            Some(AcpUpdate::Usage { used: 5, size: 9 })
        );
    }

    #[test]
    fn ignores_the_user_echo_other_sessions_non_text_and_unknown_kinds() {
        for u in [
            json!({"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"x"}}),
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"image","data":"x"}}),
            json!({"sessionUpdate":"from_the_future"}),
            json!({"sessionUpdate":"tool_call","title":"no id"}),
        ] {
            assert_eq!(parse_update(&upd(u), "s1", &none()), None);
        }
        let other = json!({"sessionId":"s2","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}}});
        assert_eq!(parse_update(&other, "s1", &none()), None);
    }

    #[test]
    fn only_once_choices_are_offered() {
        let params = json!({"sessionId":"s1","toolCall":{"toolCallId":"t"},"options":[
            {"optionId":"a","name":"Always","kind":"allow_always"},
            {"optionId":"b","name":"Allow","kind":"allow_once"},
            {"optionId":"c","name":"Never","kind":"reject_always"},
            {"optionId":"d","name":"Deny","kind":"reject_once"},
            {"optionId":"e","name":"Odd","kind":"something_else"}]});
        let (_, _, _, options) = parse_permission(&params, "s1", &none()).unwrap();
        let ids: Vec<_> = options.iter().map(|o| o.option_id.as_str()).collect();
        assert_eq!(ids, ["b", "d"]);
    }

    fn needles(values: &[&str]) -> Needles {
        Arc::new(
            values
                .iter()
                .map(|v| Zeroizing::new(v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn a_credential_straddling_the_cut_is_removed_at_every_offset() {
        let n = needles(&["TOKENVALUE-123"]);
        for pad in 0..20 {
            let text = format!("{}TOKENVALUE-123 tail", "a".repeat(pad));
            for max in 0..text.len() + 2 {
                let out = clean(&n, &text, max);
                assert!(!out.contains("TOKE"), "pad {pad} max {max}: {out}");
            }
        }
        // Redacted, then cut: the marker may be cut, the credential never shows.
        assert_eq!(clean(&n, "xxTOKENVALUE-123", 100), "xx[redacted]");
    }

    #[test]
    fn every_surfaced_string_is_redacted_after_json_decoding() {
        let n = needles(&["sec\"ret/\u{e9}tok"]);
        let tok = "sec\"ret/\u{e9}tok";
        let params = json!({"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk",
            "content":{"type":"text","text":format!("a {tok} b")},"messageId":format!("m{tok}")}});
        let shown = format!("{:?}", parse_update(&params, "s1", &n));
        assert!(
            !shown.contains("sec") && shown.contains("[redacted]"),
            "{shown}"
        );
    }
}
