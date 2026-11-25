//! Which programs may answer a remote tool's confirmation or approve a tool:
//! only the team-signed NodeSpace app (and, to approve, the team-signed
//! `nodespace` command), judged by the connecting program's code signature.
//!
//! The process gate in [`crate::services::remote_confirmation`] judges a peer by
//! its place in the process tree. That admits an orphaned descendant of a
//! recorded program and any agent the user started outside the app. Here the
//! peer is judged by what it is: on macOS the daemon reads the peer's audit
//! token when it accepts the connection (`LOCAL_PEERTOKEN`, see
//! [`crate::peer_auth`]), asks the Security framework for the running code that
//! token names (`SecCodeCopyGuestWithAttributes` with the audit attribute, so a
//! reused pid cannot match) and checks that code against a designated
//! requirement: signed by Apple's Developer ID chain, by the NodeSpace team,
//! under the expected identifier. A program that is unsigned, signed ad hoc or
//! signed by anyone else fails, whatever it is called and wherever it runs.
//!
//! **Fails closed.** No token, a token the Security framework cannot turn into
//! code, a requirement that does not parse and a check that errors are all a
//! refusal. A development build (debug assertions) refuses an unsigned app too:
//! a developer who runs the app unsigned sets `NODESPACE_DEV_ALLOW_UNSIGNED_PEER=1`
//! on the daemon, which keeps the process gate alone and logs a warning each
//! time. A release build has no such switch.
//!
//! Other platforms have no audit token; there the process gate stays the only
//! check (Windows and Linux have their own task).

use std::fmt;

/// The kernel's audit token of a peer process: eight 32-bit words that name one
/// process incarnation (uid, pid and the pid's version among them), so a pid
/// that is reused does not match it. Only macOS has one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AuditToken(pub [u32; 8]);

impl std::fmt::Debug for AuditToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The pid is word 5 of an audit token.
        write!(f, "AuditToken(pid {})", self.0[5])
    }
}

/// The NodeSpace Apple team that signs releases. Public (it is in every signed
/// binary's certificate); the release workflow signs with this team's
/// Developer ID certificates.
pub const TEAM_ID: &str = "F4M5J7YNLS";
/// The signing identifier of the desktop app (its bundle identifier).
pub const APP_IDENTIFIER: &str = "com.nodespace.desktop";
/// The signing identifier of the `nodespace` command (the file name `codesign`
/// uses for a bare executable).
pub const CLI_IDENTIFIER: &str = "nodespace";

/// What a peer asks to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Answer a remote tool's per-call confirmation: the app only.
    AnswerConfirmation,
    /// Approve a tool (`nodespace tool approve`, or the app): the app or the
    /// `nodespace` command. The person at the screen still confirms.
    ApproveTool,
}

/// Decides whether a peer, known by its audit token, may do `scope`.
pub trait PeerSignatureCheck: Send + Sync {
    fn allows(&self, token: Option<AuditToken>, scope: Scope) -> bool;
}

/// Why a peer was not judged to be the signed app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The connection carried no audit token.
    NoToken,
    /// The Security framework could not name running code for the token (the
    /// process is gone, or it is not signed).
    NoCode(String),
    /// The requirement text did not parse.
    BadRequirement(String),
    /// The code does not satisfy the requirement.
    NotTheSignedApp(String),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NoToken => write!(f, "the connection carries no audit token"),
            Refusal::NoCode(why) => write!(f, "no running code for the peer: {why}"),
            Refusal::BadRequirement(why) => write!(f, "the requirement is invalid: {why}"),
            Refusal::NotTheSignedApp(why) => {
                write!(f, "the peer is not signed as required: {why}")
            }
        }
    }
}

/// The designated requirement for a Developer ID program of `team_id` whose
/// signing identifier is `identifier`, or `None` when either holds a character
/// that does not belong in one (the text goes into a requirement string).
pub fn team_requirement(team_id: &str, identifier: &str) -> Option<String> {
    let plain = |s: &str, max: usize| {
        !s.is_empty()
            && s.len() <= max
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    };
    if !plain(team_id, 10) || !plain(identifier, 128) {
        return None;
    }
    // Apple's Developer ID chain (intermediate 1.2.840.113635.100.6.2.6, leaf
    // "Developer ID Application" 1.2.840.113635.100.6.1.13), this team, this
    // identifier.
    Some(format!(
        "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] \
and certificate leaf[field.1.2.840.113635.100.6.1.13] \
and certificate leaf[subject.OU] = \"{team_id}\" and identifier \"{identifier}\""
    ))
}

/// Check the code `token` names against the requirement text `requirement`.
#[cfg(target_os = "macos")]
pub fn check_requirement(token: Option<AuditToken>, requirement: &str) -> Result<(), Refusal> {
    use core_foundation::base::TCFType;
    use core_foundation::data::CFData;
    use security_framework::os::macos::code_signing::{
        Flags, GuestAttributes, SecCode, SecRequirement,
    };
    use std::str::FromStr;

    let token = token.ok_or(Refusal::NoToken)?;
    let requirement = SecRequirement::from_str(requirement)
        .map_err(|e| Refusal::BadRequirement(e.to_string()))?;

    let mut bytes = Vec::with_capacity(32);
    for word in token.0 {
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    let data = CFData::from_buffer(&bytes);
    let mut attributes = GuestAttributes::new();
    attributes.set_audit_token(data.as_concrete_TypeRef());

    let code = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)
        .map_err(|e| Refusal::NoCode(e.to_string()))?;
    code.check_validity(Flags::NONE, &requirement)
        .map_err(|e| Refusal::NotTheSignedApp(e.to_string()))
}

/// No audit token or Security framework off macOS.
#[cfg(not(target_os = "macos"))]
pub fn check_requirement(_token: Option<AuditToken>, _requirement: &str) -> Result<(), Refusal> {
    Err(Refusal::NoToken)
}

/// The production check: the peer must be signed by the NodeSpace team under
/// the identifier its `scope` allows. Off macOS it allows every peer (the
/// process gate stays in force there).
pub struct TeamSigned {
    app: Option<String>,
    cli: Option<String>,
}

impl TeamSigned {
    pub fn new() -> Self {
        Self::for_team(TEAM_ID, APP_IDENTIFIER, CLI_IDENTIFIER)
    }

    pub fn for_team(team_id: &str, app_identifier: &str, cli_identifier: &str) -> Self {
        Self {
            app: team_requirement(team_id, app_identifier),
            cli: team_requirement(team_id, cli_identifier),
        }
    }
}

impl Default for TeamSigned {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerSignatureCheck for TeamSigned {
    fn allows(&self, token: Option<AuditToken>, scope: Scope) -> bool {
        if !cfg!(target_os = "macos") {
            return true;
        }
        let Some(app) = self.app.as_deref() else {
            return false;
        };
        let mut verdict = check_requirement(token, app);
        if verdict.is_err() && scope == Scope::ApproveTool {
            match self.cli.as_deref() {
                Some(cli) => verdict = check_requirement(token, cli),
                None => return false,
            }
        }
        match verdict {
            Ok(()) => true,
            Err(refusal) => {
                tracing::warn!(?scope, %refusal, "a peer that is not the signed app was refused");
                false
            }
        }
    }
}

/// The name of the variable a development build reads to keep the process gate
/// alone for unsigned peers.
pub const DEV_ALLOW_UNSIGNED_PEER: &str = "NODESPACE_DEV_ALLOW_UNSIGNED_PEER";

/// A development build run with [`DEV_ALLOW_UNSIGNED_PEER`]` =1`: every peer
/// passes this check, so only the process gate judges. Not a security boundary.
#[cfg(debug_assertions)]
struct DevAllowUnsigned;

#[cfg(debug_assertions)]
impl PeerSignatureCheck for DevAllowUnsigned {
    fn allows(&self, _token: Option<AuditToken>, scope: Scope) -> bool {
        tracing::warn!(
            ?scope,
            "{DEV_ALLOW_UNSIGNED_PEER} is set: the peer's code signature is not checked \
(a development build; this is not a security boundary)"
        );
        true
    }
}

/// The check this daemon uses. A release build always checks the signature; a
/// development build does too unless [`DEV_ALLOW_UNSIGNED_PEER`] is `1`.
pub fn for_this_process() -> std::sync::Arc<dyn PeerSignatureCheck> {
    #[cfg(debug_assertions)]
    {
        if std::env::var(DEV_ALLOW_UNSIGNED_PEER).as_deref() == Ok("1") {
            return std::sync::Arc::new(DevAllowUnsigned);
        }
    }
    std::sync::Arc::new(TeamSigned::new())
}

/// Lets every peer through; for tests of what sits behind the check.
#[cfg(test)]
pub(crate) struct AllowAnyPeer;

#[cfg(test)]
impl PeerSignatureCheck for AllowAnyPeer {
    fn allows(&self, _token: Option<AuditToken>, _scope: Scope) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requirement_names_the_chain_the_team_and_the_identifier() {
        let r = team_requirement("ABCDE12345", "com.example.app").unwrap();
        assert!(r.starts_with("anchor apple generic and "));
        assert!(r.contains("certificate 1[field.1.2.840.113635.100.6.2.6]"));
        assert!(r.contains("certificate leaf[field.1.2.840.113635.100.6.1.13]"));
        assert!(r.contains("certificate leaf[subject.OU] = \"ABCDE12345\""));
        assert!(r.ends_with("identifier \"com.example.app\""));
    }

    #[test]
    fn text_that_could_change_the_requirement_is_not_put_into_one() {
        for (team, id) in [
            ("ABC\" or true or \"", "x"),
            ("ABCDE12345", "x\" or anchor apple or identifier \"y"),
            ("ABCDE123456", "x"),
            ("", "x"),
            ("ABCDE12345", ""),
            ("ABCDE12345", "a b"),
        ] {
            assert_eq!(team_requirement(team, id), None, "{team:?} {id:?}");
        }
    }

    #[test]
    fn the_release_requirements_parse() {
        // The strings go to the Security framework unchanged; a typo would
        // refuse the real app.
        #[cfg(target_os = "macos")]
        {
            use security_framework::os::macos::code_signing::SecRequirement;
            use std::str::FromStr;
            for id in [APP_IDENTIFIER, CLI_IDENTIFIER] {
                let r = team_requirement(TEAM_ID, id).unwrap();
                SecRequirement::from_str(&r).expect("the requirement parses");
            }
        }
    }

    #[test]
    fn a_bad_team_refuses_everyone_and_asks_nobody() {
        let check = TeamSigned::for_team("bad team", APP_IDENTIFIER, CLI_IDENTIFIER);
        if cfg!(target_os = "macos") {
            for scope in [Scope::AnswerConfirmation, Scope::ApproveTool] {
                assert!(!check.allows(Some(AuditToken([0; 8])), scope));
            }
        }
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use super::*;
        use crate::peer_auth::PeerStream;
        use std::process::{Child, Command, Stdio};
        use tokio::net::{UnixListener, UnixStream};

        /// The audit token of `program` as it connects to a socket of ours.
        /// `/usr/bin/nc -U` is an Apple platform binary, signed by Apple and
        /// not by the NodeSpace team.
        async fn token_of_connecting(
            program: &mut Command,
        ) -> (Option<AuditToken>, Child, PeerStream) {
            let dir = tempfile::Builder::new()
                .prefix("ns-ps-")
                .tempdir_in("/private/tmp")
                .unwrap();
            let path = dir.path().join("s.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let child = program
                .arg(&path)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let (stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept())
                    .await
                    .expect("the program connects")
                    .unwrap();
            let peer = PeerStream::new(stream);
            // The connection stays open with the caller: the program exits when it
            // closes.
            (peer.info().audit_token, child, peer)
        }

        fn nc() -> Command {
            let mut c = Command::new("/usr/bin/nc");
            c.arg("-U");
            c
        }

        #[tokio::test]
        async fn the_audit_token_is_the_connecting_process() {
            let (token, mut child, _conn) = token_of_connecting(&mut nc()).await;
            let token = token.expect("the kernel gives a token");
            assert_eq!(
                token.0[5],
                child.id(),
                "word 5 of an audit token is the pid"
            );
            let _ = child.kill();
        }

        #[tokio::test]
        async fn an_apple_signed_peer_satisfies_an_apple_requirement_and_not_the_teams() {
            let (token, mut child, _conn) = token_of_connecting(&mut nc()).await;
            // The same check, the same code path, with a requirement this peer
            // does meet: shows the check can say yes.
            check_requirement(token, "anchor apple").expect("nc is signed by Apple");
            // And the team's requirement refuses it.
            let team = team_requirement(TEAM_ID, APP_IDENTIFIER).unwrap();
            assert!(matches!(
                check_requirement(token, &team),
                Err(Refusal::NotTheSignedApp(_))
            ));
            let _ = child.kill();
        }

        #[tokio::test]
        async fn an_unsigned_or_ad_hoc_peer_is_refused_by_the_team_requirement() {
            // This test binary is signed ad hoc by the linker (or not at all):
            // it connects as itself, here through a client in the same process.
            let dir = tempfile::Builder::new()
                .prefix("ns-ps-")
                .tempdir_in("/private/tmp")
                .unwrap();
            let path = dir.path().join("s.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let _client = UnixStream::connect(&path).await.unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let token = PeerStream::new(stream).info().audit_token;
            assert!(token.is_some());
            for id in [APP_IDENTIFIER, CLI_IDENTIFIER] {
                let r = team_requirement(TEAM_ID, id).unwrap();
                assert!(
                    matches!(
                        check_requirement(token, &r),
                        Err(Refusal::NotTheSignedApp(_))
                    ),
                    "an unsigned peer must not satisfy {id}"
                );
            }
            // The production check refuses it for both scopes.
            let check = TeamSigned::new();
            assert!(!check.allows(token, Scope::AnswerConfirmation));
            assert!(!check.allows(token, Scope::ApproveTool));
        }

        #[test]
        fn no_token_is_a_refusal() {
            assert_eq!(
                check_requirement(None, "anchor apple"),
                Err(Refusal::NoToken)
            );
            let check = TeamSigned::new();
            assert!(!check.allows(None, Scope::AnswerConfirmation));
            assert!(!check.allows(None, Scope::ApproveTool));
        }

        #[test]
        fn a_token_that_names_no_process_is_a_refusal() {
            let token = Some(AuditToken([0, 0, 0, 0, 0, 0x7fff_fff0, 0, 0]));
            assert!(matches!(
                check_requirement(token, "anchor apple"),
                Err(Refusal::NoCode(_))
            ));
        }

        #[test]
        fn a_requirement_that_does_not_parse_is_a_refusal() {
            let token = Some(AuditToken([0; 8]));
            assert!(matches!(
                check_requirement(token, "this is ( not a requirement"),
                Err(Refusal::BadRequirement(_))
            ));
        }
    }
}
