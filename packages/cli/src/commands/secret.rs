//! `nodespace secret ...` — manage the credentials that remote tools refer to
//! as `keychain:<name>`.
//!
//! The value is never an argument: it is typed at a hidden prompt on a
//! terminal, or read from standard input when that is not a terminal, so it
//! never lands in shell history or a process listing. `list` prints names
//! only; there is no command that prints a value.
//!
//! The daemon asks the person at the screen to confirm a `set` or a `delete` in
//! a system dialog before it changes anything; the command says so on standard
//! error while it waits.

use std::io::{IsTerminal, Read, Write};

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use nodespace_daemon::nodespace::{DeleteSecretRequest, ListSecretNamesRequest, SetSecretRequest};
use nodespace_daemon::SecretServiceClient;
use nodespace_types::{is_secret_name, secret_value_problem, MAX_SECRET_VALUE_BYTES};
use serde_json::json;
use tonic::transport::Channel;

#[derive(Subcommand, Debug)]
pub enum SecretAction {
    /// Store a credential under a name. The value is read from a hidden prompt
    /// on a terminal, or from standard input; it is never an argument.
    Set {
        /// Name a tool refers to as `keychain:<name>` (letters, digits, `.`, `_`, `-`).
        name: String,
    },
    /// Remove a credential.
    Delete {
        /// The name to remove.
        name: String,
    },
    /// List the stored names. Values are never shown.
    ListNames,
}

/// Printed while the daemon waits for the person to confirm the change.
const CONFIRM_NOTE: &str =
    "Confirm the change in the system dialog on this Mac (Touch ID, your Watch or your password).";

pub async fn run(
    client: &mut SecretServiceClient<Channel>,
    action: SecretAction,
    json: bool,
) -> Result<()> {
    match action {
        SecretAction::Set { name } => {
            // Check the name before asking for a value.
            if !is_secret_name(&name) {
                bail!("invalid secret name: use 1 to 64 characters from letters, digits, `.`, `_` and `-`");
            }
            let value = read_value(&name)?;
            eprintln!("{CONFIRM_NOTE}");
            client
                .set(SetSecretRequest {
                    name: name.clone(),
                    value,
                })
                .await
                .map_err(|s| anyhow::anyhow!("{}", s.message()))
                .context("could not store the credential")?;
            if json {
                println!("{}", json!({ "name": name, "stored": true }));
            } else {
                println!("Stored credential `{name}`.");
            }
            Ok(())
        }
        SecretAction::Delete { name } => {
            eprintln!("{CONFIRM_NOTE}");
            let existed = client
                .delete(DeleteSecretRequest { name: name.clone() })
                .await
                .map_err(|s| anyhow::anyhow!("{}", s.message()))
                .context("could not remove the credential")?
                .into_inner()
                .existed;
            if json {
                println!("{}", json!({ "name": name, "existed": existed }));
            } else if existed {
                println!("Removed credential `{name}`.");
            } else {
                println!("No credential `{name}` was stored.");
            }
            Ok(())
        }
        SecretAction::ListNames => {
            let names = client
                .list_names(ListSecretNamesRequest {})
                .await
                .map_err(|s| anyhow::anyhow!("{}", s.message()))
                .context("could not list the credentials")?
                .into_inner()
                .names;
            if json {
                println!("{}", json!({ "names": names }));
            } else if names.is_empty() {
                println!("No credentials are stored.");
            } else {
                for name in names {
                    println!("{name}");
                }
            }
            Ok(())
        }
    }
}

/// Reads the value: a hidden prompt on a terminal, otherwise standard input.
fn read_value(name: &str) -> Result<String> {
    if std::io::stdin().is_terminal() {
        read_hidden(name)
    } else {
        read_value_from(std::io::stdin().lock())
    }
}

/// Reads a value from a stream: everything up to a bounded size, minus one
/// trailing line break. Refuses an empty value or one over the limit.
pub fn read_value_from(reader: impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_SECRET_VALUE_BYTES as u64 + 3)
        .read_to_end(&mut bytes)
        .context("could not read the value from standard input")?;
    let mut text =
        String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("the value is not valid UTF-8"))?;
    if text.ends_with('\n') {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    check_value(&text)?;
    Ok(text)
}

fn check_value(text: &str) -> Result<()> {
    match secret_value_problem(text) {
        Some(reason) => bail!("invalid secret value: {reason}"),
        None => Ok(()),
    }
}

/// Prompts on the terminal with echo off.
fn read_hidden(name: &str) -> Result<String> {
    let mut err = std::io::stderr();
    write!(err, "Value for `{name}` (hidden): ")?;
    err.flush()?;
    crossterm::terminal::enable_raw_mode()?;
    let result = read_hidden_keys();
    let _ = crossterm::terminal::disable_raw_mode();
    writeln!(err)?;
    let text = result?;
    check_value(&text)?;
    Ok(text)
}

fn read_hidden_keys() -> Result<String> {
    let mut text = String::new();
    loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match key.code {
            KeyCode::Enter => return Ok(text),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                bail!("cancelled")
            }
            KeyCode::Char(c) => {
                if text.len() + c.len_utf8() > MAX_SECRET_VALUE_BYTES {
                    bail!("the value is too long");
                }
                text.push(c);
            }
            KeyCode::Backspace => {
                text.pop();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct Harness {
        #[command(subcommand)]
        action: SecretAction,
    }

    #[test]
    fn stdin_value_loses_one_trailing_line_break_only() {
        assert_eq!(read_value_from(&b"abc\n"[..]).unwrap(), "abc");
        assert_eq!(read_value_from(&b"abc\r\n"[..]).unwrap(), "abc");
        assert_eq!(read_value_from(&b"abc"[..]).unwrap(), "abc");
        assert_eq!(read_value_from(&b"abc\n\n"[..]).unwrap(), "abc\n");
        assert_eq!(read_value_from(&b" a b "[..]).unwrap(), " a b ");
    }

    #[test]
    fn an_empty_or_oversized_or_nul_value_is_refused() {
        assert!(read_value_from(&b""[..]).is_err());
        assert!(read_value_from(&b"\n"[..]).is_err());
        assert!(read_value_from(&b"a\0b"[..]).is_err());
        let big = vec![b'x'; MAX_SECRET_VALUE_BYTES + 1];
        assert!(read_value_from(&big[..]).is_err());
        let max = vec![b'x'; MAX_SECRET_VALUE_BYTES];
        assert!(read_value_from(&max[..]).is_ok());
        assert!(read_value_from(&[0xff, 0xfe][..]).is_err());
    }

    #[test]
    fn set_takes_a_name_and_no_value_argument() {
        assert!(Harness::try_parse_from(["h", "set", "tok"]).is_ok());
        // A value on the command line would land in shell history: refused.
        assert!(Harness::try_parse_from(["h", "set", "tok", "ghp_x"]).is_err());
        assert!(Harness::try_parse_from(["h", "set", "tok", "--value", "ghp_x"]).is_err());
        assert!(Harness::try_parse_from(["h", "set", "--stdin", "tok"]).is_err());
    }

    #[test]
    fn the_value_error_never_echoes_the_value() {
        let err = read_value_from(&b"ghp_SECRET\0"[..]).unwrap_err();
        assert!(!format!("{err:#}").contains("ghp_SECRET"));
    }
}
