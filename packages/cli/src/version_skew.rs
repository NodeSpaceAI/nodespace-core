//! Detects a `nodespace` CLI and a `nodespaced` daemon that come from
//! different NodeSpace releases, and words the "update NodeSpace" advice.
//!
//! The comparison is made where it is cheap and where it helps: on a command
//! that has already failed against the daemon (an old CLI calling an RPC a new
//! daemon changed, or the reverse), and in `diagnostics`, which already makes
//! several daemon calls. A command that succeeds is never slowed down or
//! failed because the versions differ.

use nodespace_daemon::nodespace::node_service_client::NodeServiceClient;
use nodespace_daemon::nodespace::GetDaemonVersionRequest;
use nodespace_proto::with_message_limits;
use std::cmp::Ordering;
use std::time::Duration;

/// How long a failed command waits for the daemon to name its version before
/// giving up on the extra advice.
const VERSION_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

/// This CLI's own version, the workspace version it was compiled from.
pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Splits `major.minor.patch` (an optional `-pre`/`+build` suffix is ignored
/// for ordering) into numbers; `None` when it is not that shape.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// The "update NodeSpace" message for a CLI and daemon on different versions,
/// naming both; `None` when they match.
///
/// Which side is older decides the advice: an older CLI is updated through the
/// installer, an older daemon is replaced by restarting NodeSpace after an
/// update. When either version cannot be ordered the message still names both
/// and asks for the update.
pub fn skew_message(cli: &str, daemon: &str) -> Option<String> {
    let (cli, daemon) = (cli.trim(), daemon.trim());
    if cli == daemon {
        return None;
    }
    let head = format!(
        "The nodespace CLI is version {cli} but the running nodespaced daemon is version {daemon}."
    );
    let advice = match (parse(cli), parse(daemon)) {
        (Some(c), Some(d)) => match c.cmp(&d) {
            Ordering::Less => {
                "The CLI is older than the daemon: update NodeSpace so the CLI matches."
            }
            Ordering::Greater => {
                "The daemon is older than the CLI: update NodeSpace, then quit and reopen it so the new daemon starts."
            }
            Ordering::Equal => "Update NodeSpace so both come from the same release.",
        },
        _ => "Update NodeSpace so both come from the same release.",
    };
    Some(format!("{head} {advice}"))
}

/// Asks the daemon at `sock` for its version, `None` when it cannot answer
/// promptly (down, busy, or too old to know the RPC).
pub async fn daemon_version(sock: &std::path::Path) -> Option<String> {
    let lookup = async {
        let channel = crate::dial_channel(sock).await.ok()?;
        let mut client = with_message_limits!(NodeServiceClient::new(channel));
        client
            .get_daemon_version(GetDaemonVersionRequest {})
            .await
            .ok()
            .map(|response| response.into_inner().version)
    };
    tokio::time::timeout(VERSION_LOOKUP_TIMEOUT, lookup)
        .await
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_versions_say_nothing() {
        assert_eq!(skew_message("0.3.10", "0.3.10"), None);
    }

    #[test]
    fn older_cli_names_both_versions_and_asks_for_an_update() {
        let message = skew_message("0.2.8", "0.3.10").expect("mismatch");
        assert!(message.contains("0.2.8") && message.contains("0.3.10"));
        assert!(message.contains("CLI is older"));
        assert!(message.contains("update NodeSpace"));
    }

    #[test]
    fn older_daemon_asks_for_update_and_restart() {
        let message = skew_message("0.3.10", "0.3.9").expect("mismatch");
        assert!(message.contains("daemon is older"));
        assert!(message.contains("update NodeSpace"));
        assert!(message.contains("reopen"));
    }

    #[test]
    fn numeric_order_not_lexical() {
        let message = skew_message("0.3.9", "0.3.10").expect("mismatch");
        assert!(message.contains("CLI is older"));
    }

    #[test]
    fn unparseable_version_still_reports_both() {
        let message = skew_message("0.3.10", "weird").expect("mismatch");
        assert!(message.contains("weird") && message.contains("0.3.10"));
        assert!(message.contains("Update NodeSpace"));
    }

    #[test]
    fn this_cli_reports_a_parseable_version() {
        assert!(parse(CLI_VERSION).is_some());
    }
}
