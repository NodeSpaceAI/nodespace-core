//! Whether this `nodespaced` may hold an approval key: a check of its own code
//! signature when it starts.
//!
//! What protects the key is that no other program can read it or run as this
//! daemon. On macOS that holds for a release build signed with the hardened
//! runtime and no `get-task-allow` entitlement: a debugger or another process
//! of the same user cannot attach to it. So a release build makes a key only
//! when the kernel says its signature is valid (and not ad hoc, by
//! `codesign -s -` or by the linker), the hardened runtime is on, debugging is
//! not allowed and no debugger has attached (`csops(getpid(), CS_OPS_STATUS)`).
//! One that fails gets no key, and nothing is approved or offered.
//!
//! Where the key lives: a signed release that holds the team keychain access
//! group keeps it in the team keychain (see [`crate::team_keychain`]), so
//! approvals outlast a restart; every other build that makes a key keeps it in
//! memory only, and its approvals end with the process.
//!
//! A development build (debug assertions on) makes a key anyway, in memory,
//! with a loud warning: it is for working on NodeSpace and is not a security
//! boundary. Other platforms make none.

use nodespace_core::secrets::ApprovalKey;

/// `CS_VALID`: the signature is valid.
const CS_VALID: u32 = 0x0000_0001;
/// `CS_ADHOC`: signed without an identity (as the linker signs a local build).
const CS_ADHOC: u32 = 0x0000_0002;
/// `CS_GET_TASK_ALLOW`: a debugger may attach (the `get-task-allow`
/// entitlement).
const CS_GET_TASK_ALLOW: u32 = 0x0000_0004;
/// `CS_RUNTIME`: the hardened runtime is on.
const CS_RUNTIME: u32 = 0x0001_0000;
/// `CS_LINKER_SIGNED`: signed ad hoc by the linker (a local build). The kernel
/// does not set `CS_ADHOC` for these.
const CS_LINKER_SIGNED: u32 = 0x0002_0000;
/// `CS_DEBUGGED`: a debugger has attached.
const CS_DEBUGGED: u32 = 0x1000_0000;

/// The kernel's code-signing flags for a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodeStatus(pub u32);

impl CodeStatus {
    /// Signed as a release is: a valid identity signature with the hardened
    /// runtime, no debugging allowed and none attached.
    pub fn is_signed_release(self) -> bool {
        let has = |flag: u32| self.0 & flag != 0;
        has(CS_VALID)
            && has(CS_RUNTIME)
            && !has(CS_ADHOC)
            && !has(CS_LINKER_SIGNED)
            && !has(CS_GET_TASK_ALLOW)
            && !has(CS_DEBUGGED)
    }
}

/// This process's code-signing flags, or `None` when they cannot be read (or
/// the platform has none).
pub fn this_process() -> Option<CodeStatus> {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn csops(
                pid: libc::pid_t,
                ops: libc::c_uint,
                useraddr: *mut libc::c_void,
                usersize: libc::size_t,
            ) -> libc::c_int;
        }
        /// `CS_OPS_STATUS`: the process's code-signing flags.
        const CS_OPS_STATUS: libc::c_uint = 0;
        let mut flags: u32 = 0;
        // SAFETY: `flags` is a live, writable u32 of the size passed, which is
        // what CS_OPS_STATUS writes.
        let status = unsafe {
            csops(
                libc::getpid(),
                CS_OPS_STATUS,
                (&mut flags as *mut u32).cast(),
                std::mem::size_of::<u32>(),
            )
        };
        (status == 0).then_some(CodeStatus(flags))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// What the daemon does about its approval key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Make one. `warning` is set for a build that is not a security boundary.
    Make { warning: Option<String> },
    /// Make none; why, for the message an approval gets.
    None(String),
}

/// Not macOS: nothing can confirm an approval at the screen.
const NOT_THIS_PLATFORM: &str = "approving a remote tool is not available on this platform";
/// A release build whose signature check failed.
const NOT_A_SIGNED_RELEASE: &str =
    "this nodespaced is not a signed release (its code signature is missing, ad hoc, \
without the hardened runtime, or open to a debugger), so it does not approve remote tools";
/// A development build.
const DEVELOPMENT_BUILD: &str = "this nodespaced is a development build: it approves remote tools, but its \
approvals are not a security boundary (a debugger or another program of this user can read its key)";

/// The decision for a process on `macos` or not, built with debug assertions
/// or not, whose code-signing flags are `status`.
pub fn decide(macos: bool, development_build: bool, status: Option<CodeStatus>) -> Decision {
    if !macos {
        return Decision::None(NOT_THIS_PLATFORM.to_string());
    }
    if development_build {
        return Decision::Make {
            warning: Some(DEVELOPMENT_BUILD.to_string()),
        };
    }
    match status {
        Some(status) if status.is_signed_release() => Decision::Make { warning: None },
        _ => Decision::None(NOT_A_SIGNED_RELEASE.to_string()),
    }
}

/// The approval key of a daemon and whether it outlives the process.
pub struct IssuedKey {
    /// The key, or why this daemon has none.
    pub key: Result<ApprovalKey, String>,
    /// The key is kept in the team keychain, so approvals saved there still
    /// hold after a restart. False for every key held in memory only.
    pub durable: bool,
}

/// The approval key for a process whose decision is `decision`, using `team`
/// (the team keychain) when there is one. A signed release that has the team
/// keychain keeps its key there; a development build and any process without
/// the team keychain make one in memory, which ends with the process. A team
/// keychain that fails gives an in-memory key too, with a warning: the
/// approvals then end at the next restart, and nothing is trusted from a store
/// that did not answer.
pub fn approval_key_from(
    decision: Decision,
    team: Option<&dyn nodespace_core::secrets::SecretBackend>,
) -> IssuedKey {
    match decision {
        Decision::Make { warning } => {
            let release = warning.is_none();
            if let Some(warning) = warning {
                tracing::warn!("{warning}");
            }
            if let (true, Some(team)) = (release, team) {
                match nodespace_core::secrets::load_or_create_approval_key(team) {
                    Ok(key) => {
                        return IssuedKey {
                            key: Ok(key),
                            durable: true,
                        }
                    }
                    Err(e) => tracing::warn!(
                        "the team keychain gave no approval key ({e}); approvals will end when this daemon stops"
                    ),
                }
            }
            IssuedKey {
                key: ApprovalKey::generate().map_err(|e| e.to_string()),
                durable: false,
            }
        }
        Decision::None(why) => {
            tracing::warn!("{why}");
            IssuedKey {
                key: Err(why),
                durable: false,
            }
        }
    }
}

/// This daemon's approval key, made once at start, or why it has none. Logs
/// the warning of a development build and the reason a release build has no
/// key.
pub fn approval_key_for_this_process() -> IssuedKey {
    let decision = decide(
        cfg!(target_os = "macos"),
        cfg!(debug_assertions),
        this_process(),
    );
    let team = crate::team_keychain::team_backend();
    approval_key_from(decision, team.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELEASE: u32 = CS_VALID | CS_RUNTIME;

    #[test]
    fn only_a_valid_identity_signature_with_the_runtime_and_no_debugger_is_a_signed_release() {
        assert!(CodeStatus(RELEASE).is_signed_release());
        // Other flags a release carries do not matter.
        assert!(CodeStatus(RELEASE | 0x0000_0300 | 0x2000_0000).is_signed_release());
        for status in [
            0,
            CS_RUNTIME,
            CS_VALID,
            RELEASE | CS_ADHOC,
            RELEASE | CS_LINKER_SIGNED,
            RELEASE | CS_GET_TASK_ALLOW,
            RELEASE | CS_DEBUGGED,
        ] {
            assert!(!CodeStatus(status).is_signed_release(), "{status:#x}");
        }
    }

    #[test]
    fn a_key_is_made_only_on_macos_for_a_signed_release_or_a_development_build() {
        assert_eq!(
            decide(true, false, Some(CodeStatus(RELEASE))),
            Decision::Make { warning: None }
        );
        for status in [None, Some(CodeStatus(RELEASE | CS_GET_TASK_ALLOW))] {
            assert_eq!(
                decide(true, false, status),
                Decision::None(NOT_A_SIGNED_RELEASE.to_string())
            );
        }
        // A development build makes one, and says it is not a boundary.
        assert!(matches!(
            decide(true, true, None),
            Decision::Make { warning: Some(w) } if w.contains("not a security boundary")
        ));
        // Elsewhere none, whatever the build.
        for development in [false, true] {
            assert_eq!(
                decide(false, development, Some(CodeStatus(RELEASE))),
                Decision::None(NOT_THIS_PLATFORM.to_string())
            );
        }
    }

    /// The kernel answers for this process. A test binary is signed ad hoc by
    /// the linker (or not at all), never as a release.
    #[cfg(target_os = "macos")]
    #[test]
    fn this_process_reads_its_flags_and_a_test_binary_is_not_a_signed_release() {
        let status = this_process().expect("csops answers for this process");
        assert!(!status.is_signed_release(), "{:#x}", status.0);
        // On Apple silicon every binary is signed, a test binary ad hoc by the
        // linker: the flags read are the kernel's, not a default.
        #[cfg(target_arch = "aarch64")]
        assert_eq!(
            status.0 & (CS_VALID | CS_LINKER_SIGNED),
            CS_VALID | CS_LINKER_SIGNED,
            "{:#x}",
            status.0
        );
    }

    use nodespace_core::secrets::{InMemorySecretBackend, SecretBackend, SecretError};

    fn release() -> Decision {
        Decision::Make { warning: None }
    }

    fn development() -> Decision {
        Decision::Make {
            warning: Some("dev".into()),
        }
    }

    /// Fails every call; panics if it is touched when it must not be.
    struct Failing;
    impl SecretBackend for Failing {
        fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
            Err(SecretError::Store("down".into()))
        }
        fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
            Err(SecretError::Store("down".into()))
        }
        fn delete(&self, _: &str) -> Result<bool, SecretError> {
            Err(SecretError::Store("down".into()))
        }
    }

    #[test]
    fn a_signed_release_with_the_team_keychain_keeps_the_same_key_across_restarts() {
        let team = InMemorySecretBackend::default();
        let first = approval_key_from(release(), Some(&team));
        let second = approval_key_from(release(), Some(&team));
        assert!(first.durable && second.durable);
        assert_eq!(
            first.key.unwrap().as_bytes(),
            second.key.unwrap().as_bytes()
        );
    }

    #[test]
    fn a_development_build_never_reads_or_writes_the_team_keychain_and_keeps_no_key() {
        let team = InMemorySecretBackend::default();
        team.set(
            nodespace_core::secrets::APPROVAL_KEY_ACCOUNT,
            &"ab".repeat(32),
        )
        .unwrap();
        let before = team.snapshot();
        let a = approval_key_from(development(), Some(&team));
        let b = approval_key_from(development(), Some(&team));
        assert!(!a.durable && !b.durable);
        assert_ne!(a.key.unwrap().as_bytes(), b.key.unwrap().as_bytes());
        assert_eq!(team.snapshot(), before);
    }

    #[test]
    fn a_process_without_the_team_keychain_makes_a_new_key_each_time() {
        let a = approval_key_from(release(), None);
        let b = approval_key_from(release(), None);
        assert!(!a.durable && !b.durable);
        assert_ne!(a.key.unwrap().as_bytes(), b.key.unwrap().as_bytes());
    }

    #[test]
    fn a_team_keychain_that_fails_or_holds_a_planted_item_gives_a_memory_key() {
        let failed = approval_key_from(release(), Some(&Failing));
        assert!(!failed.durable && failed.key.is_ok());
        let planted = InMemorySecretBackend::default();
        planted
            .set(nodespace_core::secrets::APPROVAL_KEY_ACCOUNT, "planted")
            .unwrap();
        let issued = approval_key_from(release(), Some(&planted));
        assert!(!issued.durable && issued.key.is_ok());
        // The item is left as it was.
        assert_eq!(
            planted
                .get(nodespace_core::secrets::APPROVAL_KEY_ACCOUNT)
                .unwrap()
                .as_deref(),
            Some("planted")
        );
    }

    #[test]
    fn no_decision_to_make_a_key_gives_none_and_touches_nothing() {
        let team = InMemorySecretBackend::default();
        let issued = approval_key_from(Decision::None("no".into()), Some(&team));
        assert_eq!(issued.key.err().as_deref(), Some("no"));
        assert!(!issued.durable);
        assert!(team.snapshot().is_empty());
    }
}
