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

/// Status codes a version difference can plausibly produce: an RPC the other
/// side lacks or changed, or a request it cannot decode. Everything else
/// (not found, already exists, permission denied, ...) is the user's own
/// error and never gets the update advice.
pub fn may_come_from_skew(code: tonic::Code) -> bool {
    use tonic::Code::{FailedPrecondition, Internal, InvalidArgument, Unimplemented, Unknown};
    matches!(
        code,
        Unimplemented | InvalidArgument | Unknown | Internal | FailedPrecondition
    )
}

/// The first gRPC status code in an error's chain.
pub fn status_code(error: &anyhow::Error) -> Option<tonic::Code> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<tonic::Status>())
        .map(tonic::Status::code)
}

/// What `diagnostics` shows for the daemon's version: the version when it
/// answered, and a warning when it differs from `cli` or could not be read.
/// A daemon too old to report its version is a reason to update, not a
/// failure of the report.
pub fn diagnostics_version(
    cli: &str,
    daemon: Result<String, &tonic::Status>,
) -> (Option<String>, Option<String>) {
    match daemon {
        Ok(version) => {
            let warning = skew_message(cli, &version);
            (Some(version), warning)
        }
        Err(status) => (
            None,
            Some(format!(
                "The daemon's version could not be read ({:?}); it may predate version reporting. \
                 The nodespace CLI is version {cli}: update NodeSpace so both come from the same release.",
                status.code()
            )),
        ),
    }
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

/// The update advice for `error` when the daemon is on `daemon` and the CLI on
/// `cli`: only for a status that may come from skew, `None` otherwise.
pub fn skew_note(error: &anyhow::Error, cli: &str, daemon: &str) -> Option<String> {
    if !status_code(error).is_some_and(may_come_from_skew) {
        return None;
    }
    skew_message(cli, daemon)
}

/// An error with advice after it. It prints the wrapped error's own message
/// first, so the real error stays the headline, then the note; the wrapped
/// error's causes stay its causes. The error is kept, not turned into text.
#[derive(Debug)]
struct WithNote {
    error: anyhow::Error,
    note: String,
}

impl std::fmt::Display for WithNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}\n\n{}", self.error, self.note)
    }
}

impl std::error::Error for WithNote {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.chain().nth(1)
    }
}

/// `error` with `note` after its own message; see [`WithNote`].
pub fn with_note(error: anyhow::Error, note: String) -> anyhow::Error {
    anyhow::Error::new(WithNote { error, note })
}

/// The error `main` prints for a failed command: the required-extensions
/// refusal and the status text are rendered first, as for every other failure,
/// and the update advice, when the daemon is on another release and the status
/// may come from skew, follows. The lookup runs only for such a status, so
/// other failures pay nothing.
pub async fn render_failure(
    error: anyhow::Error,
    sock: &std::path::Path,
    cli: &str,
) -> anyhow::Error {
    let candidate = status_code(&error).is_some_and(may_come_from_skew);
    let note = if candidate {
        match daemon_version(sock).await {
            Some(daemon) => skew_note(&error, cli, &daemon),
            None => None,
        }
    } else {
        None
    };
    let rendered = crate::render_status(crate::render_refusal(error));
    match note {
        Some(note) => with_note(rendered, note),
        None => rendered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Marker;
    impl std::fmt::Display for Marker {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "marker")
        }
    }
    impl std::error::Error for Marker {}

    #[test]
    fn a_noted_error_keeps_its_causes_and_prints_its_message_first() {
        let error = anyhow::Error::new(Marker)
            .context("outer")
            .context("outermost");
        let noted = with_note(error, "ADVICE".into());
        let text = format!("{noted:?}");
        assert!(text.starts_with("outermost\n\nADVICE"), "{text}");
        assert!(text.contains("outer") && text.contains("marker"), "{text}");
        assert!(
            noted
                .chain()
                .any(|cause| cause.downcast_ref::<Marker>().is_some()),
            "a downcastable cause must survive the note"
        );
    }

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

    fn status_error(code: tonic::Code) -> anyhow::Error {
        anyhow::Error::new(tonic::Status::new(code, "boom")).context("while doing it")
    }

    #[test]
    fn mismatch_adds_advice_after_the_original_error() {
        let error = status_error(tonic::Code::Unimplemented);
        let note = skew_note(&error, "0.2.8", "0.3.10").expect("advice");
        let error = with_note(crate::render_status(error), note);
        let text = format!("{error:?}");
        assert!(text.contains("boom"), "{text}");
        assert!(text.contains("0.2.8") && text.contains("0.3.10"), "{text}");
        assert!(
            text.starts_with("while doing it\n\n"),
            "the original error must be the headline: {text}"
        );
    }

    #[test]
    fn matching_versions_leave_the_error_alone() {
        let error = status_error(tonic::Code::Internal);
        assert_eq!(skew_note(&error, "0.3.10", "0.3.10"), None);
    }

    #[test]
    fn an_error_that_is_not_a_status_is_untouched() {
        let error = anyhow::anyhow!("bad argument");
        assert_eq!(skew_note(&error, "0.2.8", "0.3.10"), None);
    }

    #[test]
    fn only_codes_that_can_come_from_skew_get_the_advice() {
        use tonic::Code::*;
        for code in [
            Unimplemented,
            InvalidArgument,
            Unknown,
            Internal,
            FailedPrecondition,
        ] {
            let error = status_error(code);
            assert!(skew_note(&error, "0.2.8", "0.3.10").is_some(), "{code:?}");
        }
        for code in [
            NotFound,
            AlreadyExists,
            PermissionDenied,
            Unauthenticated,
            Unavailable,
            Cancelled,
        ] {
            let error = status_error(code);
            assert!(skew_note(&error, "0.2.8", "0.3.10").is_none(), "{code:?}");
        }
    }

    #[test]
    fn diagnostics_warns_on_mismatch_and_stays_quiet_on_match() {
        let (version, warning) = diagnostics_version("0.3.10", Ok("0.3.9".into()));
        assert_eq!(version.as_deref(), Some("0.3.9"));
        assert!(warning.expect("mismatch warns").contains("0.3.9"));
        assert_eq!(diagnostics_version("0.3.10", Ok("0.3.10".into())).1, None);
    }

    #[test]
    fn diagnostics_treats_an_unreadable_version_as_a_warning() {
        let status = tonic::Status::unimplemented("no such rpc");
        let (version, warning) = diagnostics_version("0.3.10", Err(&status));
        assert_eq!(version, None);
        let warning = warning.expect("unknown version warns");
        assert!(warning.contains("predate") && warning.contains("update NodeSpace"));
    }

    #[test]
    fn this_cli_reports_a_parseable_version() {
        assert!(parse(CLI_VERSION).is_some());
    }
}
