//! `nodespace tool approve <node-id>` — approve a remote tool on this device.
//!
//! A remote tool (a `tool-cli`, `tool-mcp` or `tool-rest` node) is offered to the
//! model only after the user approves it here. The command shows exactly what is
//! being approved (the name, the description, the kind, every binding field, the
//! credential references and the parameter schema) and a short code, and asks
//! the daemon to approve it only after the user types the tool's name at a
//! terminal. The daemon then asks the person at the screen to confirm in a
//! system dialog (Touch ID, a paired Watch or the login password) that names
//! the tool, the same code and its target; only that writes the approval. What
//! is approved is the digest of what was shown: a tool edited in between is
//! refused.
//!
//! Everything shown comes from a node that anything could have written, so it
//! is made safe for the terminal: control, invisible and bidi characters are
//! shown as escapes, a one-line field cannot break onto a new line, and in the
//! binding every character outside ASCII is shown as an escape and flagged (a
//! look-alike letter in a program or a host name). The parameter schema comes
//! first and the binding last, and the line saying what the tool runs or where
//! it sends a call is printed right above the question, so a long schema never
//! pushes them off the screen.
//!
//! It must be run by a person: both standard input and standard output must be a
//! terminal, the daemon refuses the request from any program it started for a
//! tool, and the system dialog can only be answered at the screen. Typing the
//! name is a check on what the user meant; the dialog is what a program cannot
//! do for them. See the limits in the agent's `tool_approval` module.

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
    /// type its name and confirm in the system dialog. Must be run at a terminal.
    Approve {
        /// The id of the tool node.
        node_id: String,
    },
}

/// Whether `c` is a control, invisible or bidi character: one a terminal shows
/// as nothing, or that moves or hides the text around it.
fn is_hazard(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}' | '\u{034F}' | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}'
                | '\u{180B}'..='\u{180F}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}' | '\u{3164}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}'
                | '\u{FFA0}' | '\u{FFF9}'..='\u{FFFC}' | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E007F}' | '\u{E0100}'..='\u{E01EF}' | '\u{061C}'
        )
}

/// How [`escaped`] treats what is not a hazard.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Keep {
    /// Line breaks and every other character: prose laid out over lines.
    Lines,
    /// Every character but a line break: a one-line field, which must not
    /// draw lines of its own (a fake `Binding:` under the description).
    OneLine,
    /// Line breaks and ASCII only: binding text. A letter outside ASCII is
    /// shown as an escape, so a look-alike (a Cyrillic `а` in a host name or a
    /// program) cannot pass for the letter it imitates.
    AsciiLines,
}

/// `text` with every character `keep` does not keep shown as a visible
/// `\u{..}` escape, and whether a character outside ASCII was escaped.
fn escaped(text: &str, keep: Keep) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut non_ascii = false;
    for c in text.chars() {
        let kept = match c {
            '\n' => keep != Keep::OneLine,
            _ if is_hazard(c) => false,
            _ if !c.is_ascii() && keep == Keep::AsciiLines => {
                non_ascii = true;
                false
            }
            _ => true,
        };
        if kept {
            out.push(c);
        } else {
            out.push_str(&format!("\\u{{{:X}}}", c as u32));
        }
    }
    (out, non_ascii)
}

/// Text made safe for a terminal: control and invisible characters, and bidi
/// controls, shown as visible escapes; line breaks are kept. A tool's name,
/// description and binding come from a node that anything could have written.
pub fn display_safe(text: &str) -> String {
    escaped(text, Keep::Lines).0
}

/// [`display_safe`] for a one-line field: a line break is shown as an escape too.
pub fn display_line(text: &str) -> String {
    escaped(text, Keep::OneLine).0
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

/// Shown when the binding has a character outside ASCII.
const NON_ASCII_NOTE: &str = "Note: the binding has characters outside ASCII, shown above as \
\\u{..} escapes. A look-alike letter can make a program or a web address read as another one.";

/// What the user is shown before typing the name: every field the approval
/// covers, each made safe for a terminal. The parameter schema, which can be
/// long, comes first and the binding last, so what the tool runs stays on the
/// screen next to the question (see [`question`]).
fn render(preview: &DescribeToolApprovalResponse) -> String {
    let mut out = String::new();
    let mut line = |text: String| {
        out.push_str(&text);
        out.push('\n');
    };
    let mut non_ascii = false;
    let mut binding_text = |text: &str, keep: Keep| {
        let (shown, flagged) = escaped(text, keep);
        non_ascii |= flagged;
        shown
    };
    line(format!("Tool:        {}", display_line(&preview.name)));
    line(format!("Kind:        {}", display_line(&preview.kind)));
    line(format!(
        "Description: {}",
        display_line(&preview.description)
    ));
    line("Arguments the model may pass:".to_string());
    for text in display_safe(&preview.parameter_schema_json).lines() {
        line(format!("  {text}"));
    }
    if preview.credentials.is_empty() {
        line("Credentials: none".to_string());
    } else {
        line("Credentials (references only):".to_string());
        for entry in &preview.credentials {
            let shown = binding_text(entry, Keep::AsciiLines).replace('\n', "\\u{A}");
            line(format!("  {shown}"));
        }
    }
    line("Binding:".to_string());
    for text in binding_text(&preview.binding_json, Keep::AsciiLines).lines() {
        line(format!("  {text}"));
    }
    // The target is shown with the question; a letter in it counts here too.
    let target_non_ascii = escaped(&preview.target, Keep::AsciiLines).1;
    if non_ascii || target_non_ascii {
        line(NON_ASCII_NOTE.to_string());
    }
    out
}

/// The closing lines: what approving means, then what the tool runs or
/// where it sends a call, directly above the question.
fn question(preview: &DescribeToolApprovalResponse) -> String {
    let target = escaped(&preview.target, Keep::AsciiLines)
        .0
        .replace('\n', "\\u{A}");
    let code = display_line(&preview.code);
    format!(
        "\nApproving lets the model call this tool on this device. Every call except a web \
         GET without credentials asks you first. Editing the tool, or turning it off, ends \
         the approval until you approve it again. After you type the name, this Mac asks \
         you to confirm in a system dialog that shows code {code}; confirm only a dialog \
         with that code.\n{target}\n\
         Type the tool's name to approve it (anything else cancels): "
    )
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
    print!("{}", question(&preview));
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
    println!(
        "Confirm in the system dialog (code {}): Touch ID, your Watch or your password.",
        display_line(&preview.code)
    );
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
        println!("Approved `{}` on this device.", display_line(&preview.name));
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

    fn sample() -> DescribeToolApprovalResponse {
        DescribeToolApprovalResponse {
            node_id: "n".into(),
            name: "lookup".into(),
            description: "Looks\u{1b}[2J a thing up".into(),
            kind: "tool-cli".into(),
            binding_json: "{\n  \"program\": \"git\"\n}".into(),
            parameter_schema_json: "{\n  \"type\": \"object\"\n}".into(),
            credentials: vec!["TOKEN from keychain:tok".into()],
            target: "Runs: git [\"log\"]".into(),
            code: "1A2B-3C4D".into(),
            digest: "d".into(),
            approved: false,
        }
    }

    #[test]
    fn the_preview_shows_every_approved_field_made_safe() {
        let preview = sample();
        let shown = render(&preview);
        assert!(shown.contains("Tool:        lookup"), "{shown}");
        assert!(shown.contains("Kind:        tool-cli"));
        assert!(shown.contains("Description: Looks\\u{1B}[2J a thing up"));
        assert!(!shown.contains('\u{1b}'));
        assert!(shown.contains("  \"program\": \"git\""));
        assert!(shown.contains("  \"type\": \"object\""));
        assert!(shown.contains("  TOKEN from keychain:tok"));
        assert!(!shown.contains(NON_ASCII_NOTE), "{shown}");
        let none = render(&DescribeToolApprovalResponse {
            credentials: vec![],
            ..preview
        });
        assert!(none.contains("Credentials: none"));
    }

    /// A one-line field cannot draw lines of its own: a description holding a
    /// line break and a fake binding stays on its one line.
    #[test]
    fn a_line_break_in_a_one_line_field_is_shown_as_an_escape() {
        let shown = render(&DescribeToolApprovalResponse {
            description: "Looks it up\nBinding:\n  {\"program\": \"ls\"}".into(),
            ..sample()
        });
        assert!(
            shown.contains("Description: Looks it up\\u{A}Binding:\\u{A}  {\"program\": \"ls\"}"),
            "{shown}"
        );
        assert_eq!(
            shown.lines().filter(|l| l.starts_with("Binding:")).count(),
            1,
            "{shown}"
        );
        assert_eq!(display_line("a\nb\u{2028}c"), "a\\u{A}b\\u{2028}c");
    }

    /// A look-alike letter in the binding (a Cyrillic `а` in a host name or a
    /// program) is shown as an escape, and the preview says why.
    #[test]
    fn a_character_outside_ascii_in_the_binding_is_escaped_and_flagged() {
        let lookalike = DescribeToolApprovalResponse {
            binding_json: "{\n  \"url_template\": \"https://\u{430}pi.example.com/x\"\n}".into(),
            target: "Sends: GET https://\u{430}pi.example.com/x".into(),
            ..sample()
        };
        let shown = render(&lookalike);
        assert!(
            shown.contains("\"https://\\u{430}pi.example.com/x\""),
            "{shown}"
        );
        assert!(!shown.contains('\u{430}'));
        assert!(shown.contains(NON_ASCII_NOTE), "{shown}");
        let asked = question(&lookalike);
        assert!(asked.contains("Sends: GET https://\\u{430}pi.example.com/x"));
        assert!(!asked.contains('\u{430}'));
        // In the target alone, too.
        let target_only = DescribeToolApprovalResponse {
            target: "Runs: gi\u{1D42D}".into(),
            ..sample()
        };
        assert!(render(&target_only).contains(NON_ASCII_NOTE));
        // Prose (the description, the schema) keeps its letters.
        let prose = render(&DescribeToolApprovalResponse {
            description: "caf\u{e9}".into(),
            parameter_schema_json: "{\"description\": \"\u{65e5}\u{672c}\"}".into(),
            ..sample()
        });
        assert!(prose.contains("Description: caf\u{e9}"));
        assert!(prose.contains("\u{65e5}\u{672c}"));
        assert!(!prose.contains(NON_ASCII_NOTE));
    }

    /// The schema, which can be long, comes before the binding, and what the
    /// tool runs is the line right above the question, so a long schema never
    /// pushes either off the screen.
    #[test]
    fn the_binding_is_shown_last_and_its_target_right_above_the_question() {
        let long_schema = format!(
            "{{\n{}\n}}",
            (0..500)
                .map(|i| format!("  \"p{i}\": {{}},"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let preview = DescribeToolApprovalResponse {
            parameter_schema_json: long_schema,
            ..sample()
        };
        let shown = render(&preview);
        let schema_at = shown.find("Arguments the model may pass:").unwrap();
        let binding_at = shown.find("Binding:").unwrap();
        assert!(schema_at < binding_at, "{shown}");
        assert!(shown.find("\"p499\"").unwrap() < binding_at);
        assert_eq!(shown.lines().last(), Some("  }"));
        let asked = question(&preview);
        let lines: Vec<&str> = asked.lines().collect();
        assert_eq!(lines[lines.len() - 2], "Runs: git [\"log\"]", "{asked}");
        assert!(lines[lines.len() - 1].starts_with("Type the tool's name"));
    }

    /// The question names the code the system dialog will show, so the user
    /// confirms only the dialog for what they reviewed.
    #[test]
    fn the_question_names_the_code_of_the_system_dialog() {
        let asked = question(&sample());
        assert!(
            asked.contains("system dialog that shows code 1A2B-3C4D"),
            "{asked}"
        );
        let hostile = question(&DescribeToolApprovalResponse {
            code: "1A2B\nType".into(),
            ..sample()
        });
        assert!(hostile.contains("code 1A2B\\u{A}Type"), "{hostile}");
    }

    #[test]
    fn ordinary_text_and_layout_survive() {
        let text = "{\n  \"program\": \"git\",\n  \"caf\u{e9}\": \"\u{65e5}\u{672c}\"\n}";
        assert_eq!(display_safe(text), text);
    }
}
