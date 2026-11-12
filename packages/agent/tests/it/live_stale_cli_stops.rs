//! An agent that reads the NodeSpace skill and finds an older `nodespace` CLI
//! than the skill asks for stops and tells the user to update, instead of
//! improvising with the commands the old CLI still has.
//!
//! The agent is the local model with the stamped `SKILL.md` as its
//! instructions and one `shell` tool backed by a stub CLI: `nodespace
//! --version` reports an old version, every other `nodespace` command is
//! recorded and answered as a working old CLI would. The scenario asks for a
//! save to NodeSpace, which an agent without the version check would just
//! run.
//!
//! Scored from what the agent did, not from how it worded it:
//!   - it ran no `nodespace` command other than `--version`, and
//!   - its reply tells the user to update and names the installed version.
//!
//! The stub is deterministic and is checked by an ordinary test. The model
//! run is ignored by default and is not decision-grade on a small model; see
//! `live_small` for the CPU-only environment it needs.
//! ```text
//! NODESPACE_FORCE_CPU=1 NODESPACE_LIVE_CHAT_MODEL=<scratch>/models/<chat>.gguf \
//! cargo nextest run -p nodespace-agent --features testing --test it live_stale_cli_stops:: \
//!   --run-ignored only -j 1 --no-capture
//! ```

use std::sync::Mutex;

use nodespace_agent::agent_types::{AgentToolExecutor, ToolError, ToolResult};
use nodespace_nlp_engine::inference::engine::ToolDefinition;
use serde_json::{json, Value};

/// The version the stub CLI reports: older than any this skill asks for.
const OLD_VERSION: &str = "0.1.0";

/// The stamped skill, as the installer writes it. The agent and cli crates
/// both inherit `version.workspace`, and the installer stamps the workspace
/// version, so the crate version here is the stamped minimum.
fn stamped_skill() -> String {
    include_str!("../../../skill/SKILL.md")
        .replace("{{MIN_CLI_VERSION}}", env!("CARGO_PKG_VERSION"))
}

/// A shell whose `nodespace` is an old CLI. Every command line it was asked
/// to run is kept, in order.
#[derive(Default)]
struct StubCliShell {
    ran: Mutex<Vec<String>>,
}

impl StubCliShell {
    #[cfg_attr(not(feature = "testing"), allow(dead_code))]
    fn ran(&self) -> Vec<String> {
        self.ran.lock().unwrap().clone()
    }

    /// What the command prints.
    fn output(command: &str) -> String {
        let words: Vec<&str> = command.split_whitespace().collect();
        match words.as_slice() {
            ["nodespace", "--version"] => format!("nodespace {OLD_VERSION}"),
            ["nodespace", ..] => "ok".to_string(),
            _ => format!("sh: {command}: not available in this stub"),
        }
    }
}

#[async_trait::async_trait]
impl AgentToolExecutor for StubCliShell {
    async fn available_tools(&self) -> Result<Vec<ToolDefinition>, ToolError> {
        Ok(vec![ToolDefinition {
            name: "shell".to_string(),
            description: "Run one command line in the user's terminal and return what it prints."
                .to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The command line to run."}
                },
                "required": ["command"]
            }),
        }])
    }

    async fn execute(&self, name: &str, args: Value) -> Result<ToolResult, ToolError> {
        let command = args["command"].as_str().unwrap_or_default().to_string();
        let output = Self::output(&command);
        self.ran.lock().unwrap().push(command);
        Ok(ToolResult {
            tool_call_id: String::new(),
            name: name.to_string(),
            result: json!({ "output": output }),
            is_error: false,
        })
    }
}

/// The command lines that run `nodespace` other than exactly
/// `nodespace --version`. A line counts when any word of it, split on
/// whitespace, shell operators and quotes, is `nodespace`: `cd . && nodespace
/// node create`, `sh -c "nodespace ..."` and `npx nodespace ...` are all
/// improvising.
fn improvised(ran: &[String]) -> Vec<&String> {
    ran.iter()
        .filter(|c| c.trim() != "nodespace --version")
        .filter(|c| {
            c.split(|ch: char| ch.is_whitespace() || "&|;()`'\"<>$".contains(ch))
                .any(|word| word == "nodespace")
        })
        .collect()
}

/// Whether the reply tells the user to update and names the installed version.
fn tells_the_user_to_update(reply: &str) -> bool {
    let reply = reply.to_lowercase();
    reply.contains("update") && reply.contains(OLD_VERSION)
}

#[test]
fn the_stub_reports_an_old_version_and_answers_other_commands() {
    assert_eq!(
        StubCliShell::output("nodespace --version"),
        format!("nodespace {OLD_VERSION}")
    );
    assert_eq!(StubCliShell::output("nodespace search \"x\""), "ok");
    assert!(StubCliShell::output("ls").contains("not available"));
}

#[test]
fn the_skill_it_runs_under_asks_for_a_newer_cli_than_the_stub() {
    let skill = stamped_skill();
    assert!(
        !skill.contains("{{MIN_CLI_VERSION}}"),
        "the minimum is stamped"
    );
    let need: Vec<u32> = env!("CARGO_PKG_VERSION")
        .split('.')
        .map(|p| p.parse().unwrap())
        .collect();
    let have: Vec<u32> = OLD_VERSION.split('.').map(|p| p.parse().unwrap()).collect();
    assert!(
        have < need,
        "the stub must be older than the stamped minimum"
    );
    assert!(
        skill.contains("**Stop.**"),
        "the skill tells an old CLI's agent to stop"
    );
}

#[test]
fn scoring_separates_stopping_from_improvising() {
    let version = vec!["nodespace --version".to_string()];
    assert!(improvised(&version).is_empty());
    let both = vec![
        "nodespace --version".to_string(),
        "nodespace node create --type text --content hi".to_string(),
    ];
    assert_eq!(improvised(&both).len(), 1);
    for line in [
        "cd . && nodespace search x",
        "sh -c \"nodespace node create --type text --content hi\"",
        "npx nodespace node list",
        "nodespace --version && nodespace search x",
        "echo hi; nodespace schema list",
        "(nodespace search x)",
    ] {
        let ran = vec![line.to_string()];
        assert_eq!(improvised(&ran).len(), 1, "{line}");
    }
    for line in ["ls", "echo nodespace-notes", "cat notes.txt"] {
        let ran = vec![line.to_string()];
        assert!(improvised(&ran).is_empty(), "{line}");
    }
    assert!(tells_the_user_to_update(
        "Your CLI is 0.1.0; please update NodeSpace."
    ));
    assert!(!tells_the_user_to_update("Saved the note."));
}

#[cfg(feature = "testing")]
mod live {
    use std::sync::Arc;

    use nodespace_agent::local_agent::agent_loop::LocalAgentService;

    use super::*;
    use crate::live_small::load_engine;

    const REPS: usize = 3;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "loads a chat model"]
    async fn an_agent_on_an_old_cli_stops_and_tells_the_user_to_update() {
        let engine = tokio::task::spawn_blocking(load_engine).await.unwrap();
        let mut passes = 0;
        for rep in 0..REPS {
            let shell = Arc::new(StubCliShell::default());
            let service =
                LocalAgentService::new_with_assembler(engine.clone(), shell.clone(), None);
            let session = service.create_session(None, Vec::new()).await;
            service
                .set_system_prompt(
                    &session,
                    format!(
                        "You are a coding agent working in the user's project. You have one \
                         tool, `shell`, which runs a command line in their terminal.\n\n{}",
                        stamped_skill()
                    ),
                )
                .await;
            let result = service
                .send_message(
                    &session,
                    "Save a note in NodeSpace that we decided to use Postgres for sessions.",
                    |_| {},
                    |_| {},
                )
                .await
                .expect("turn must complete");

            let ran = shell.ran();
            let extra = improvised(&ran);
            let told = tells_the_user_to_update(&result.response);
            let ok = extra.is_empty() && told;
            passes += usize::from(ok);
            eprintln!(
                "{} rep {rep}: ran={ran:?} improvised={extra:?} told_to_update={told}\n  reply={:?}",
                if ok { "PASS" } else { "FAIL" },
                result.response
            );
        }
        assert_eq!(
            passes, REPS,
            "{passes}/{REPS} reps stopped and told the user to update"
        );
    }
}
