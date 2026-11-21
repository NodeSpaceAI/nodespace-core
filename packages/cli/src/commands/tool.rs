//! `nodespace tool approve <node-id>` — approve a remote tool on this device.
//!
//! A remote tool (a `tool-cli`, `tool-mcp` or `tool-rest` node) is offered to the
//! model only after the user approves it here. The command shows exactly what is
//! being approved (the name, the description, the kind, every binding field, the
//! credential references and the parameter schema) and writes the approval only
//! after the user types the tool's name at a terminal. What is approved is the
//! digest of what was shown: a tool edited in between is refused.
//!
//! It must be run by a person: both standard input and standard output must be a
//! terminal, and the daemon refuses the request from any program it started for a
//! tool, so a tool the model runs cannot approve itself or another tool.

use std::io::{IsTerminal, Write};

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use nodespace_daemon::nodespace::{
    ApproveToolRequest, DescribeToolApprovalRequest, DescribeToolApprovalResponse,
};
use serde_json::json;

use crate::LocalAgentClient;

#[derive(Subcommand, Debug)]
pub enum ToolAction {
    /// Show what approving a remote tool would approve, and approve it after you
    /// type its name. Must be run at a terminal.
    Approve {
        /// The id of the tool node.
        node_id: String,
    },
}

/// Text made safe for a terminal: control and invisible characters, and bidi
/// controls, shown as visible escapes. A tool's name, description and binding
/// come from a node that anything could have written.
pub fn display_safe(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let unsafe_char = c.is_control()
            || matches!(
                c,
                '\u{00AD}' | '\u{034F}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}'
                    | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}'
                    | '\u{2060}'..='\u{206F}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}'
                    | '\u{FFA0}' | '\u{FFF9}'..='\u{FFFC}' | '\u{1D173}'..='\u{1D17A}'
                    | '\u{E0000}'..='\u{E007F}' | '\u{E0100}'..='\u{E01EF}' | '\u{061C}'
            );
        match c {
            // A line break inside indented JSON is the layout, not a hazard.
            '\n' => out.push('\n'),
            _ if unsafe_char => out.push_str(&format!("\\u{{{:X}}}", c as u32)),
            _ => out.push(c),
        }
    }
    out
}

/// Refuses unless both standard input and standard output are a terminal: an
/// approval is given by a person typing at one, and a program a tool runs has
/// none. Checked before the daemon is contacted.
pub fn require_terminal() -> Result<()> {
    check_terminal(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
    )
}

fn check_terminal(stdin_is_terminal: bool, stdout_is_terminal: bool) -> Result<()> {
    if !(stdin_is_terminal && stdout_is_terminal) {
        bail!(
            "approving a tool must be done at a terminal: run this command yourself in a terminal window"
        );
    }
    Ok(())
}

/// What the user is shown before typing the name: every field the approval
/// covers, each made safe for a terminal.
fn render(preview: &DescribeToolApprovalResponse) -> String {
    let mut out = String::new();
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };
    line(format!("Tool:        {}", display_safe(&preview.name)));
    line(format!("Kind:        {}", display_safe(&preview.kind)));
    line(format!(
        "Description: {}",
        display_safe(&preview.description)
    ));
    line("Binding:".to_string());
    for text in display_safe(&preview.binding_json).lines() {
        line(format!("  {text}"));
    }
    line("Arguments the model may pass:".to_string());
    for text in display_safe(&preview.parameter_schema_json).lines() {
        line(format!("  {text}"));
    }
    if preview.credentials.is_empty() {
        line("Credentials: none".to_string());
    } else {
        line("Credentials (references only):".to_string());
        for entry in &preview.credentials {
            line(format!("  {}", display_safe(entry)));
        }
    }
    out
}

/// Whether the line the user typed is the tool's name, exactly (only the line
/// break is dropped).
fn typed_the_name(answer: &str, name: &str) -> bool {
    !name.is_empty() && answer.trim_end_matches(['\r', '\n']) == name
}

pub async fn run(client: &mut LocalAgentClient, action: ToolAction, json: bool) -> Result<()> {
    match action {
        ToolAction::Approve { node_id } => approve(client, &node_id, json).await,
    }
}

async fn approve(client: &mut LocalAgentClient, node_id: &str, json: bool) -> Result<()> {
    require_terminal()?;
    let preview = client
        .describe_tool_approval(DescribeToolApprovalRequest {
            node_id: node_id.to_string(),
        })
        .await
        .map_err(|s| anyhow::anyhow!("{}", s.message()))
        .context("could not read the tool")?
        .into_inner();

    print!("{}", render(&preview));
    if preview.approved {
        println!("\nThis tool is already approved on this device.");
        return Ok(());
    }
    println!(
        "\nApproving lets the model call this tool on this device. Every call except a web \
         GET without credentials asks you first. Editing the tool later turns it off again \
         until you approve it again."
    );
    print!("Type the tool's name to approve it (anything else cancels): ");
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("could not read your answer")?;
    if !typed_the_name(&answer, &preview.name) {
        if json {
            println!("{}", json!({ "approved": false }));
        } else {
            println!("Not approved.");
        }
        return Ok(());
    }
    client
        .approve_tool(ApproveToolRequest {
            node_id: node_id.to_string(),
            digest: preview.digest,
        })
        .await
        .map_err(|s| anyhow::anyhow!("{}", s.message()))
        .context("the tool was not approved")?;
    if json {
        println!("{}", json!({ "approved": true, "tool": preview.name }));
    } else {
        println!("Approved `{}` on this device.", display_safe(&preview.name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_invisible_and_bidi_characters_are_shown_as_escapes() {
        assert_eq!(display_safe("a\u{1b}[31mb"), "a\\u{1B}[31mb");
        assert_eq!(
            display_safe("harmless\u{202E}gnp.exe"),
            "harmless\\u{202E}gnp.exe"
        );
        assert_eq!(display_safe("a\u{200B}b\u{E0041}"), "a\\u{200B}b\\u{E0041}");
        assert_eq!(display_safe("a\rb\0c"), "a\\u{D}b\\u{0}c");
    }

    #[test]
    fn it_refuses_without_a_terminal_on_either_side() {
        assert!(check_terminal(true, true).is_ok());
        for (stdin, stdout) in [(false, true), (true, false), (false, false)] {
            let err = check_terminal(stdin, stdout).unwrap_err().to_string();
            assert!(err.contains("at a terminal"), "{err}");
        }
    }

    #[test]
    fn only_the_exact_name_approves() {
        assert!(typed_the_name("lookup\n", "lookup"));
        assert!(typed_the_name("lookup\r\n", "lookup"));
        assert!(typed_the_name("lookup", "lookup"));
        for wrong in [
            "",
            "\n",
            "y\n",
            "yes\n",
            "Lookup\n",
            " lookup\n",
            "lookup \n",
            "look\n",
        ] {
            assert!(!typed_the_name(wrong, "lookup"), "{wrong:?}");
        }
        assert!(!typed_the_name("\n", ""));
    }

    #[test]
    fn the_preview_shows_every_approved_field_made_safe() {
        let preview = DescribeToolApprovalResponse {
            node_id: "n".into(),
            name: "lookup".into(),
            description: "Looks\u{1b}[2J a thing up".into(),
            kind: "tool-cli".into(),
            binding_json: "{\n  \"program\": \"git\"\n}".into(),
            parameter_schema_json: "{\n  \"type\": \"object\"\n}".into(),
            credentials: vec!["TOKEN from keychain:tok".into()],
            digest: "d".into(),
            approved: false,
        };
        let shown = render(&preview);
        assert!(shown.contains("Tool:        lookup"), "{shown}");
        assert!(shown.contains("Kind:        tool-cli"));
        assert!(shown.contains("Description: Looks\\u{1B}[2J a thing up"));
        assert!(!shown.contains('\u{1b}'));
        assert!(shown.contains("  \"program\": \"git\""));
        assert!(shown.contains("  \"type\": \"object\""));
        assert!(shown.contains("  TOKEN from keychain:tok"));
        let none = render(&DescribeToolApprovalResponse {
            credentials: vec![],
            ..preview
        });
        assert!(none.contains("Credentials: none"));
    }

    #[test]
    fn ordinary_text_and_layout_survive() {
        let text = "{\n  \"program\": \"git\",\n  \"caf\u{e9}\": \"\u{65e5}\u{672c}\"\n}";
        assert_eq!(display_safe(text), text);
    }
}
