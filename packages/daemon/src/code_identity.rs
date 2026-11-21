//! Whether this `nodespaced` may hold an approval key: a check of its own code
//! signature when it starts.
//!
//! The approval key lives only in the daemon's memory, so what protects it is
//! that no other program can read that memory or run as this daemon. On macOS
//! that holds for a release build signed with the hardened runtime and no
//! `get-task-allow` entitlement: a debugger or another process of the same user
//! cannot attach to it. So a release build makes a key only when the kernel
//! says its signature is valid (and not ad hoc), the hardened runtime is on,
//! debugging is not allowed and no debugger has attached
//! (`csops(getpid(), CS_OPS_STATUS)`). One that fails gets no key, and nothing
//! is approved or offered.
//!
//! A development build (debug assertions on) makes a key anyway, with a loud
//! warning: it is for working on NodeSpace and is not a security boundary.
//! Other platforms make none.

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

/// This daemon's approval key, made once at start, or why it has none. Logs
/// the warning of a development build and the reason a release build has no
/// key.
pub fn approval_key_for_this_process() -> Result<ApprovalKey, String> {
    match decide(
        cfg!(target_os = "macos"),
        cfg!(debug_assertions),
        this_process(),
    ) {
        Decision::Make { warning } => {
            if let Some(warning) = warning {
                tracing::warn!("{warning}");
            }
            ApprovalKey::generate().map_err(|e| e.to_string())
        }
        Decision::None(why) => {
            tracing::warn!("{why}");
            Err(why)
        }
    }
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
    }
}
