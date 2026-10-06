//! The golden case-file format: everything the model sees, as data.
//!
//! A case file is TOML declaring the *pre-template* inputs — system prompt,
//! tool definitions, optional history, user message. Those are the four
//! ADR-064 instruction channels, and they are what a case exists to reword.
//! The chat template below them is fixed machinery the runner drives through
//! production's own engine, so it is not restated here.
//!
//! No assertions and no scoring: see [`super`] for why.
//!
//! ## Shape
//!
//! ```toml
//! name = "indirect-reference-resolves-without-asking-for-an-id"
//! reps = 3
//!
//! [[turn]]
//! system = """
//! You are a graph-editing assistant.
//! """
//! user = "The 2400 one came back — set it to returned"
//!
//!   [[turn.history]]
//!   role = "system"
//!   content = "Fact: one equipment-item node was created."
//!
//!   [[turn.tool]]
//!   name = "resolve_query"
//!   description = "Resolve an indirect reference to the node it refers to."
//!   parameters_schema = { type = "object", properties = { request = { type = "string" } } }
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::agent_types::ToolDefinition;
use nodespace_nlp_engine::chat::{ChatMessage, Role};

/// A parsed case file: one or more turns, run `reps` times as a whole.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenCase {
    /// Identifier for this case, used in output. Defaults to the file stem.
    #[serde(default)]
    pub name: String,
    /// Free-text note recording what the case is probing. Never sent to the
    /// model — it documents the case for whoever reads the file next.
    #[serde(default)]
    pub notes: String,
    /// How many independent runs of the whole case to perform.
    ///
    /// Deliberately not `Option`: a case that omits it still gets
    /// [`DEFAULT_REPS`], because a single run is not decision-grade. Identical
    /// code has scored 6, 7, and 7 out of 12 on the agent matrix across three
    /// runs, and scenario 6's `resolve_query` behavior measured 3/3 one way
    /// and 0/8 the other depending on carried history.
    #[serde(default = "default_reps")]
    pub reps: u32,
    /// Sampling temperature for every turn.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// Cap on generated tokens per turn.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// The turns, in order. A single-turn case has exactly one.
    ///
    /// Turns always chain: each one sees the real user/assistant/tool exchange
    /// of every turn before it. That is right for a conversation, and wrong
    /// for a file holding several independent probes — those would each read
    /// the previous probe's exchange. Put independent probes in separate
    /// files.
    #[serde(rename = "turn")]
    pub turns: Vec<Turn>,
}

/// Reps applied when a case file omits `reps`.
pub const DEFAULT_REPS: u32 = 3;

fn default_reps() -> u32 {
    DEFAULT_REPS
}

fn default_temperature() -> f32 {
    0.1
}

fn default_max_tokens() -> u32 {
    512
}

/// One turn: what the model is shown.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    /// Optional label for this turn in output. Defaults to `turn<N>`.
    #[serde(default)]
    pub label: String,
    /// The system prompt for this turn. Each turn carries its own, because
    /// the production loop's two stages do not share one.
    pub system: String,
    /// Literal history messages, after the system prompt and before anything
    /// carried forward from earlier turns.
    #[serde(default, rename = "history")]
    pub history: Vec<HistoryMessage>,
    /// The user message for this turn.
    pub user: String,
    /// What this turn was tuned to produce, in plain prose.
    ///
    /// **Never evaluated.** The runner prints it next to what the model
    /// actually did and stops there; comparing the two is the reader's job.
    /// It exists because the case files are the deliverable — a tuned case
    /// handed to the snapshot gate or the guidance work weeks later has to
    /// answer "what was this tuned to produce?" from the file itself, not
    /// from whoever tuned it.
    ///
    /// Deliberately a free-text string rather than a typed expectation: the
    /// moment it has structure, something will be tempted to assert on it,
    /// and a second scoring system is what this utility exists to not be.
    #[serde(default)]
    pub expect: String,
    /// The tool surface offered on this turn.
    #[serde(default, rename = "tool")]
    pub tools: Vec<CaseTool>,
    /// Canned results for the tool calls this turn makes, keyed by tool name.
    ///
    /// In a multi-turn case, each tool call this turn emitted is carried into
    /// the next turn as an assistant tool-call turn paired with its result, so
    /// the following turn sees a well-formed exchange rather than an orphan
    /// tool result. A tool with no entry here gets [`DEFAULT_TOOL_RESULT`].
    ///
    /// Keyed by tool name, so a turn calling the same tool twice replays the
    /// same result for both. Each call still gets its own paired message —
    /// only the content is shared. Distinct per-call results would need a
    /// different key, which no case has wanted.
    #[serde(default)]
    pub tool_results: BTreeMap<String, String>,
}

/// The stand-in result for a tool call the case file did not supply one for.
///
/// Deliberately contentless: a case that depends on what a tool returned
/// should say so in `tool_results` rather than inherit a fact from the
/// harness. This exists only so the replayed exchange is well-formed.
pub const DEFAULT_TOOL_RESULT: &str = "{\"ok\": true}";

/// A history message as written in the case file.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryMessage {
    /// `system`, `user`, `assistant`, or `tool`.
    pub role: CaseRole,
    /// The message text.
    pub content: String,
}

/// Message roles as spelled in a case file. Mirrors [`Role`] rather than
/// reusing it so the case format is not silently redefined by a change to the
/// engine's enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaseRole {
    /// A system message.
    System,
    /// A user message.
    User,
    /// An assistant message.
    Assistant,
    /// A tool result message.
    Tool,
}

impl From<CaseRole> for Role {
    fn from(r: CaseRole) -> Self {
        match r {
            CaseRole::System => Role::System,
            CaseRole::User => Role::User,
            CaseRole::Assistant => Role::Assistant,
            CaseRole::Tool => Role::Tool,
        }
    }
}

impl HistoryMessage {
    /// Convert to the engine's message type.
    pub fn to_chat_message(&self) -> ChatMessage {
        ChatMessage::text(self.role.into(), self.content.clone())
    }
}

/// A tool definition as written in a case file.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseTool {
    /// Tool name the model calls.
    pub name: String,
    /// Tool description — under ADR-064 rule 2 this channel owns tool
    /// selection, so it is one of the things a case exists to tune.
    pub description: String,
    /// JSON Schema for the tool's parameters, written as an inline TOML table.
    #[serde(default = "empty_object_schema")]
    pub parameters_schema: toml::Value,
}

fn empty_object_schema() -> toml::Value {
    let mut t = toml::map::Map::new();
    t.insert("type".into(), toml::Value::String("object".into()));
    toml::Value::Table(t)
}

impl CaseTool {
    /// Convert to the inference boundary's tool type.
    ///
    /// The schema round-trips TOML → JSON via serde rather than being
    /// hand-walked, so any JSON Schema construct TOML can express survives
    /// without this function knowing the schema's shape.
    pub fn to_tool_definition(&self) -> Result<ToolDefinition, CaseError> {
        let parameters_schema = serde_json::to_value(&self.parameters_schema)
            .map_err(|e| CaseError::Schema(self.name.clone(), e.to_string()))?;
        Ok(ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters_schema,
        })
    }
}

/// Strip the indentation a readable TOML file adds to the text the model sees.
///
/// Load-bearing, not tidiness. Rust's `"…\` continuation eats the newline
/// *and* the next line's leading whitespace; TOML's `"""` does not. A block
/// indented for readability therefore reaches the model with a leading
/// newline, a per-line indent, a doubled space at each continuation, and a
/// trailing indent — none of which the author wrote or can see. Measured
/// against the Rust goldens these were ported from, one history block came out
/// 35 characters longer than the literal it reproduces.
///
/// The premise of the whole utility is that the case file is exactly what the
/// model receives, so the loader owns the gap rather than asking every author
/// to write column-0 TOML and remember why.
///
/// Only the *common* indent is removed and space-collapsing applies only
/// within a line, so blank lines and deliberate relative indentation (a
/// numbered list, a nested clause) survive — narrative shape is sometimes the
/// very thing a case is comparing.
pub fn normalize_case_text(raw: &str) -> String {
    // Drop the blank first line that `"""` leaves when it opens on its own
    // line. `"\r\n"` is tried first because it is the longer prefix — the
    // conventional order, though here the two are equivalent, since `.lines()`
    // below strips a trailing `\r` from every line anyway and the only job
    // left for this strip is removing the empty leading line itself.
    let body = raw
        .strip_prefix("\r\n")
        .or_else(|| raw.strip_prefix('\n'))
        .unwrap_or(raw);

    // Indentation is counted in CHARACTERS, never bytes. `char::is_whitespace`
    // accepts multibyte spaces — U+00A0, U+2003, U+3000 — which arrive by
    // pasting prompt text out of a chat window, and a byte offset taken from
    // one of them lands mid-character. Slicing there panics, and the panic
    // would name no case file, so the author's next move would be to suspect
    // their prompt edit rather than this function.
    let leading_ws_chars = |l: &str| l.chars().take_while(|c| c.is_whitespace()).count();

    // Over non-blank lines only: a blank line carries no indentation evidence,
    // and counting it as zero would defeat dedenting for every block that
    // contains a paragraph break.
    let indent = body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(leading_ws_chars)
        .min()
        .unwrap_or(0);

    body.lines()
        .map(|line| {
            // Skipping N chars is boundary-safe by construction where slicing
            // N bytes is not. A blank line shorter than the common indent just
            // yields nothing, which is what it should.
            let dedented: String = line.chars().skip(indent).collect();
            // A line's own remaining leading whitespace is deliberate relative
            // structure the dedent just preserved, so only interior runs
            // collapse.
            let lead: String = dedented.chars().take_while(|c| c.is_whitespace()).collect();
            let mut out = String::with_capacity(dedented.len());
            out.push_str(&lead);
            let mut pending_space = false;
            for ch in dedented.chars().skip(lead.chars().count()) {
                if ch == ' ' {
                    pending_space = true;
                    continue;
                }
                if pending_space {
                    out.push(' ');
                    pending_space = false;
                }
                out.push(ch);
            }
            out.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

/// Errors from loading a case file.
#[derive(Debug)]
pub enum CaseError {
    /// The file could not be read.
    Io(String),
    /// The file was not valid TOML, or did not match the case schema.
    Parse(String),
    /// A tool's `parameters_schema` could not be converted to JSON.
    Schema(String, String),
    /// The case is structurally invalid (e.g. no turns).
    Invalid(String),
}

impl std::fmt::Display for CaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaseError::Io(m) => write!(f, "could not read case file: {m}"),
            CaseError::Parse(m) => write!(f, "case file is not a valid golden case: {m}"),
            CaseError::Schema(tool, m) => {
                write!(
                    f,
                    "tool '{tool}' has an unconvertible parameters_schema: {m}"
                )
            }
            CaseError::Invalid(m) => write!(f, "case is invalid: {m}"),
        }
    }
}

impl std::error::Error for CaseError {}

impl GoldenCase {
    /// Parse a case from TOML text, defaulting `name` to `default_name`.
    pub fn from_toml(text: &str, default_name: &str) -> Result<Self, CaseError> {
        let mut case: GoldenCase =
            toml::from_str(text).map_err(|e| CaseError::Parse(e.to_string()))?;
        if case.name.is_empty() {
            case.name = default_name.to_string();
        }
        case.normalize();
        case.validate()?;
        Ok(case)
    }

    /// Load and parse a case file from disk.
    pub fn load(path: &Path) -> Result<Self, CaseError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| CaseError::Io(format!("{}: {e}", path.display())))?;
        let default_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("case")
            .to_string();
        Self::from_toml(&text, &default_name)
    }

    /// Apply [`normalize_case_text`] to every string the model will see, plus
    /// `expect` — which it will not, but which is printed beside the result
    /// and so wants the same indentation stripped.
    ///
    /// `notes` is excluded: it is read as a block of prose in the file itself,
    /// where its own paragraph layout is the point. Tool *names* are excluded
    /// too — they are identifiers compared exactly against what the model
    /// emits, and whitespace-collapsing an identifier would hide a typo rather
    /// than fix one.
    fn normalize(&mut self) {
        for turn in &mut self.turns {
            turn.system = normalize_case_text(&turn.system);
            turn.user = normalize_case_text(&turn.user);
            turn.expect = normalize_case_text(&turn.expect);
            for h in &mut turn.history {
                h.content = normalize_case_text(&h.content);
            }
            for tool in &mut turn.tools {
                tool.description = normalize_case_text(&tool.description);
            }
            for result in turn.tool_results.values_mut() {
                *result = normalize_case_text(result);
            }
        }
    }

    fn validate(&self) -> Result<(), CaseError> {
        if self.turns.is_empty() {
            return Err(CaseError::Invalid(
                "a case must declare at least one [[turn]]".into(),
            ));
        }
        if self.reps == 0 {
            return Err(CaseError::Invalid("reps must be at least 1".into()));
        }
        // Tool schemas are converted eagerly so a malformed one is reported
        // before the 5GB model load, not several minutes into it.
        for turn in &self.turns {
            for tool in &turn.tools {
                tool.to_tool_definition()?;
            }
        }
        Ok(())
    }

    /// The label for turn `index`, defaulting to `turn<N>`.
    pub fn turn_label(&self, index: usize) -> String {
        let turn = &self.turns[index];
        if turn.label.is_empty() {
            format!("turn{}", index + 1)
        } else {
            turn.label.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_CASE: &str = r#"
reps = 2

[[turn]]
system = "You are a test assistant."
user = "do the thing"

  [[turn.tool]]
  name = "do_thing"
  description = "Does the thing."
  parameters_schema = { type = "object", properties = { what = { type = "string" } } }
"#;

    #[test]
    fn parses_a_minimal_case_and_defaults_its_name_to_the_file_stem() {
        let case = GoldenCase::from_toml(MINIMAL_CASE, "my-case").expect("case must parse");
        assert_eq!(case.name, "my-case");
        assert_eq!(case.reps, 2);
        assert_eq!(case.turns.len(), 1);
        assert_eq!(case.turn_label(0), "turn1");
    }

    #[test]
    fn reps_defaults_to_three_when_omitted() {
        let text = MINIMAL_CASE.replace("reps = 2\n", "");
        let case = GoldenCase::from_toml(&text, "c").expect("case must parse");
        assert_eq!(
            case.reps, DEFAULT_REPS,
            "a case that omits reps must still run more than once — single runs are not \
             decision-grade"
        );
    }

    #[test]
    fn tool_schema_round_trips_from_toml_to_json() {
        let case = GoldenCase::from_toml(MINIMAL_CASE, "c").expect("case must parse");
        let def = case.turns[0].tools[0]
            .to_tool_definition()
            .expect("schema must convert");
        assert_eq!(def.name, "do_thing");
        assert_eq!(def.parameters_schema["type"], "object");
        assert_eq!(
            def.parameters_schema["properties"]["what"]["type"],
            "string"
        );
    }

    #[test]
    fn rejects_an_unknown_field_rather_than_silently_ignoring_it() {
        // A typo'd key would otherwise read as "the tuning change had no
        // effect", the worst failure mode for a tool whose job is showing what
        // a prompt edit did.
        let text = MINIMAL_CASE.replace("reps = 2", "reps = 2\nrepetitions = 5");
        let err = GoldenCase::from_toml(&text, "c").expect_err("must reject");
        assert!(matches!(&err, CaseError::Parse(m) if m.contains("repetitions")));
    }

    #[test]
    fn an_indented_toml_block_reaches_the_model_as_the_author_wrote_it() {
        // The exact shape the committed case files use: `"""` opening on its
        // own line, body indented for readability, `\` continuations joining
        // indented lines, closing delimiter indented. Rust's `"…\` produces
        // the flat string; TOML alone does not.
        let text = r#"
[[turn]]
system = """
    You are a graph-editing assistant. When a request refers to something \
    indirectly (a bare value, a description) rather than by name, call \
    resolve_query to find it. Never ask the user to supply a node id yourself.
    """
user = "u"
  [[turn.tool]]
  name = "t"
  description = """
  Resolve an indirect reference (a bare value, a relative date, a description) \
  to the single node it refers to.
  """
"#;
        let case = GoldenCase::from_toml(text, "c").expect("case must parse");
        assert_eq!(
            case.turns[0].system,
            "You are a graph-editing assistant. When a request refers to something \
             indirectly (a bare value, a description) rather than by name, call \
             resolve_query to find it. Never ask the user to supply a node id yourself.",
            "the model must see exactly the prompt the case author wrote — no leading \
             newline, no indent, no doubled spaces at continuations, no trailing indent"
        );
        assert_eq!(
            case.turns[0].tools[0].description,
            "Resolve an indirect reference (a bare value, a relative date, a description) \
             to the single node it refers to."
        );
    }

    #[test]
    fn normalization_preserves_paragraphs_and_relative_indentation() {
        // A case probing narrative-shaped history depends on this: the shape
        // IS the variable, so flattening it would destroy the comparison.
        let raw =
            "\n  Intro paragraph.\n\n  Then a list:\n    1. First item.\n    2. Second item.\n  ";
        assert_eq!(
            normalize_case_text(raw),
            "Intro paragraph.\n\nThen a list:\n  1. First item.\n  2. Second item.",
            "only the COMMON indent is stripped — relative indentation and blank lines \
             are content"
        );
    }

    #[test]
    fn normalization_survives_multibyte_leading_whitespace() {
        // `char::is_whitespace` accepts U+3000 (3 bytes), U+2003, and U+00A0.
        // Counting the indent in bytes and then slicing lands mid-character
        // and panics. These arrive by pasting prompt text out of a chat
        // window, and the likeliest carrier is an invisible one: a "blank"
        // line made of ideographic spaces between normally-indented lines.
        // One U+3000 vs two spaces: the common indent is 1 CHARACTER, so
        // `beta` keeps one space of relative indentation. That is the correct
        // answer for genuinely mixed indentation — the point here is that it
        // produces an answer at all rather than panicking mid-character.
        assert_eq!(
            normalize_case_text("\n\u{3000}alpha\n  beta\n"),
            "alpha\n beta",
            "a multibyte-indented line must normalize, not panic"
        );
        assert_eq!(
            normalize_case_text("  alpha\n\u{3000}\u{3000}\n  beta"),
            "alpha\n\nbeta",
            "a blank line of ideographic spaces is filtered from the indent \
             computation but still gets sliced — that is the reachable panic"
        );
        assert_eq!(normalize_case_text("\u{00a0}x"), "x", "NBSP indent");
        assert_eq!(normalize_case_text("\u{2003}x"), "x", "em-space indent");
    }

    #[test]
    fn normalization_handles_crlf_tabs_and_degenerate_input() {
        assert_eq!(normalize_case_text("\r\n  a\r\n  b"), "a\nb");
        assert_eq!(normalize_case_text("\t\ta\n\t\tb"), "a\nb");
        assert_eq!(normalize_case_text(""), "");
        assert_eq!(normalize_case_text("\n"), "");
        assert_eq!(normalize_case_text("   "), "");
    }

    #[test]
    fn normalization_never_leaves_a_carriage_return_in_the_prompt() {
        // A CRLF-authored case file must not smuggle a carriage return into
        // the prompt, where it would be invisible in every editor and diff.
        // The guarantee comes from `.lines()`, which strips a trailing `\r`
        // per line — not from the opening `strip_prefix`, whose only job is
        // removing the blank first line. Asserted over inputs that put a CRLF
        // at the head, in the middle, and at the tail.
        for raw in [
            "\r\n  a\r\n  b",
            "\r\nsolo",
            "\r\n\r\n  a",
            "  a\r\n  b\r\n",
        ] {
            let out = normalize_case_text(raw);
            assert!(
                !out.contains('\r'),
                "carriage return survived normalization of {raw:?} -> {out:?}"
            );
        }
        assert_eq!(normalize_case_text("\r\n  a\r\n  b"), "a\nb");
    }

    #[test]
    fn normalization_preserves_multibyte_content_mid_line() {
        // The committed cases carry an em-dash in the user message. Content
        // characters must survive untouched — only leading whitespace is the
        // normalizer's business.
        let raw = "\n  The 2400 one came back — set it to returned\n  ";
        assert_eq!(
            normalize_case_text(raw),
            "The 2400 one came back — set it to returned"
        );
    }

    #[test]
    fn normalization_is_idempotent_and_leaves_flat_text_untouched() {
        let flat = "Fact: one equipment-item node was created.";
        assert_eq!(normalize_case_text(flat), flat);
        let once = normalize_case_text("\n  a b\n  c\n  ");
        assert_eq!(normalize_case_text(&once), once);
    }

    #[test]
    fn tool_results_are_normalized_so_replayed_json_stays_parseable() {
        let text = r#"
[[turn]]
system = "s"
user = "u"
  [turn.tool_results]
  t = """
  {"ok": true, "note": "done"}
  """
  [[turn.tool]]
  name = "t"
  description = "d"
"#;
        let case = GoldenCase::from_toml(text, "c").expect("case must parse");
        let result = &case.turns[0].tool_results["t"];
        assert_eq!(result, r#"{"ok": true, "note": "done"}"#);
        serde_json::from_str::<serde_json::Value>(result).expect("replayed result must be JSON");
    }

    #[test]
    fn rejects_a_case_with_no_turns() {
        let err = GoldenCase::from_toml("reps = 1\n", "c").expect_err("must reject");
        assert!(matches!(err, CaseError::Parse(_) | CaseError::Invalid(_)));
    }

    #[test]
    fn rejects_zero_reps() {
        let text = MINIMAL_CASE.replace("reps = 2", "reps = 0");
        let err = GoldenCase::from_toml(&text, "c").expect_err("must reject");
        assert!(matches!(err, CaseError::Invalid(m) if m.contains("reps")));
    }
}
