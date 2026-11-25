//! The team keychain as the daemon uses it: where credentials, the approval key
//! and the saved approvals live in a signed release that holds the team's
//! keychain access group, and when it is not used at all.
//!
//! A program of the user can write to the login keychain with `security`, and
//! can plant an item there that a daemon would trust. The data-protection
//! keychain under a team access group cannot be reached that way: the operating
//! system lets only a program whose signature carries the group in its
//! `keychain-access-groups` entitlement (backed by the team's provisioning
//! profile) read or write it.
//!
//! **Which store a daemon uses is decided by what it is, not by what happens to
//! work.** A daemon that is a signed release (see [`crate::code_identity`]) and
//! holds the group in its own entitlements ([`holds_group`], read from its
//! signature) uses the team keychain for credentials, their names index, the
//! approval key and the saved approvals, and a failure of that keychain is an
//! error (no fallback to the login keychain, so a transient failure cannot make
//! the daemon trust a planted item). Every other daemon (a development build, an
//! ad hoc or Homebrew build, a release signed without the profile) keeps the
//! login keychain for credentials and an approval key in memory only: its
//! approvals end with the process.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use nodespace_core::secrets::{SecretBackend, SecretError, SecretStore, TeamKeychainBackend};
use nodespace_core::tool_approvals::{PersistedRecord, RecordSink};

/// The keychain access group of the daemon: the team's id, a dot, and the
/// group's name. The release signs `nodespaced` with
/// `keychain-access-groups = [ "<team id>.app.nodespace.daemon" ]`
/// (`scripts/pkg-resources/nodespaced.entitlements`).
pub const ACCESS_GROUP: &str = "F4M5J7YNLS.app.nodespace.daemon";

/// Where this daemon keeps what it must protect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The team keychain: durable, and out of reach of other programs.
    Team,
    /// The login keychain for credentials, nothing durable for approvals.
    Login,
}

/// The mode of a daemon that is (or is not) a signed release and does (or does
/// not) hold the access group.
pub fn mode_for(signed_release: bool, holds_group: bool) -> Mode {
    if signed_release && holds_group {
        Mode::Team
    } else {
        Mode::Login
    }
}

/// Whether this process's signature carries `group` in its
/// `keychain-access-groups` entitlement.
#[cfg(target_os = "macos")]
pub fn holds_group(group: &str) -> bool {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::string::{CFString, CFStringRef};
    use std::ffi::c_void;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SecTaskCreateFromSelf(allocator: *const c_void) -> *const c_void;
        fn SecTaskCopyValueForEntitlement(
            task: *const c_void,
            entitlement: CFStringRef,
            error: *mut *const c_void,
        ) -> *const c_void;
    }

    // SAFETY: both functions follow the create rule: a non-null result is owned
    // by the caller and is released when its wrapper drops. The task outlives
    // the call that reads it. A null error pointer is allowed.
    unsafe {
        let task = SecTaskCreateFromSelf(std::ptr::null());
        if task.is_null() {
            return false;
        }
        let task = CFType::wrap_under_create_rule(task);
        let name = CFString::from_static_string("keychain-access-groups");
        let value = SecTaskCopyValueForEntitlement(
            task.as_CFTypeRef(),
            name.as_concrete_TypeRef(),
            std::ptr::null_mut(),
        );
        if value.is_null() {
            return false;
        }
        let Some(groups) = CFType::wrap_under_create_rule(value).downcast_into::<CFArray>() else {
            return false;
        };
        let names: Vec<String> = groups
            .iter()
            .filter_map(|item| {
                // An item of the array is borrowed from it (the get rule).
                CFType::wrap_under_get_rule(*item as *const _)
                    .downcast::<CFString>()
                    .map(|s| s.to_string())
            })
            .collect();
        groups_include(&names, group)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn holds_group(_group: &str) -> bool {
    false
}

/// Whether the entitlement's groups include `group`, exactly: not a prefix, a
/// longer name or a wildcard.
pub fn groups_include(groups: &[String], group: &str) -> bool {
    groups.iter().any(|g| g == group)
}

/// This daemon's mode, decided once.
pub fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        let signed_release = !cfg!(debug_assertions)
            && crate::code_identity::this_process().is_some_and(|s| s.is_signed_release());
        mode_for(signed_release, holds_group(ACCESS_GROUP))
    })
}

/// The team keychain, when this daemon is to use it.
pub fn team_backend() -> Option<Arc<dyn SecretBackend>> {
    (mode() == Mode::Team)
        .then(|| Arc::new(TeamKeychainBackend::new(ACCESS_GROUP)) as Arc<dyn SecretBackend>)
}

/// The store for credentials and their names index: the team keychain in
/// [`Mode::Team`], the login keychain otherwise.
pub fn credential_store() -> SecretStore {
    match team_backend() {
        Some(backend) => SecretStore::new(backend),
        None => SecretStore::keychain(),
    }
}

/// The saved approvals, as one item of the team keychain per state directory:
/// two daemons of the same user with different homes keep their own, so one
/// cannot overwrite what the other saved or ended. The item holds a generation
/// that only goes up, so a copy older than what was last written is never
/// restored over it.
pub struct RecordsInKeychain {
    backend: Arc<dyn SecretBackend>,
    account: String,
    generation: AtomicU64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedApprovals {
    generation: u64,
    records: Vec<PersistedRecord>,
}

/// A short name for a state directory, for the item's account.
pub fn home_tag(state_dir: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(state_dir.as_os_str().as_encoded_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

impl RecordsInKeychain {
    /// The saved approvals of the daemon whose state directory is tagged `home`
    /// (see [`home_tag`]).
    pub fn new(backend: Arc<dyn SecretBackend>, home: &str) -> Self {
        Self {
            backend,
            account: format!(
                "{}:{home}",
                nodespace_core::secrets::APPROVAL_RECORDS_ACCOUNT
            ),
            generation: AtomicU64::new(0),
        }
    }

    fn stored(&self) -> Result<Option<SavedApprovals>, String> {
        match self.backend.get(&self.account).map_err(why)? {
            None => Ok(None),
            Some(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|_| "the saved approvals are not readable".to_string()),
        }
    }
}

fn why(error: SecretError) -> String {
    error.to_string()
}

impl RecordSink for RecordsInKeychain {
    fn load(&self) -> Result<Vec<PersistedRecord>, String> {
        match self.stored()? {
            None => Ok(Vec::new()),
            Some(saved) => {
                self.generation.store(saved.generation, Ordering::SeqCst);
                Ok(saved.records)
            }
        }
    }

    fn save(&self, records: &[PersistedRecord]) -> Result<(), String> {
        // Above whatever is stored now, so a later copy always reads as newer.
        let stored = self.stored().ok().flatten().map_or(0, |s| s.generation);
        let generation = stored.max(self.generation.load(Ordering::SeqCst)) + 1;
        let text = serde_json::to_string(&SavedApprovals {
            generation,
            records: records.to_vec(),
        })
        .map_err(|_| "the approvals cannot be written".to_string())?;
        self.backend.set(&self.account, &text).map_err(why)?;
        self.generation.store(generation, Ordering::SeqCst);
        Ok(())
    }

    fn clear(&self) -> Result<(), String> {
        self.backend.delete(&self.account).map(|_| ()).map_err(why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nodespace_core::secrets::InMemorySecretBackend;

    #[test]
    fn only_a_signed_release_that_holds_the_group_uses_the_team_keychain() {
        assert_eq!(mode_for(true, true), Mode::Team);
        for (signed, group) in [(true, false), (false, true), (false, false)] {
            assert_eq!(mode_for(signed, group), Mode::Login, "{signed} {group}");
        }
    }

    #[test]
    fn a_test_binary_holds_no_group_and_uses_the_login_keychain() {
        assert!(!holds_group(ACCESS_GROUP));
        assert!(!holds_group("anything"));
        assert_eq!(mode(), Mode::Login);
        assert!(team_backend().is_none());
    }

    fn record(version: i64) -> PersistedRecord {
        PersistedRecord {
            scope: "db".into(),
            node_id: "n".into(),
            tag: "t".into(),
            program_sha256: None,
            version,
            created_at: chrono::Utc::now(),
        }
    }

    fn account(home: &str) -> String {
        format!(
            "{}:{home}",
            nodespace_core::secrets::APPROVAL_RECORDS_ACCOUNT
        )
    }

    #[test]
    fn saved_approvals_round_trip_and_garbage_is_an_error() {
        let backend = Arc::new(InMemorySecretBackend::default());
        let sink = RecordsInKeychain::new(backend.clone(), "h");
        assert_eq!(sink.load().unwrap(), Vec::new());
        let r = record(3);
        sink.save(std::slice::from_ref(&r)).unwrap();
        assert_eq!(sink.load().unwrap(), vec![r]);
        backend.set(&account("h"), "{").unwrap();
        assert!(sink.load().is_err());
    }

    #[test]
    fn each_state_directory_has_its_own_item_and_clear_removes_only_its_own() {
        let backend = Arc::new(InMemorySecretBackend::default());
        let a = RecordsInKeychain::new(backend.clone(), &home_tag(std::path::Path::new("/h/a")));
        let b = RecordsInKeychain::new(backend.clone(), &home_tag(std::path::Path::new("/h/b")));
        assert_ne!(
            home_tag(std::path::Path::new("/h/a")),
            home_tag(std::path::Path::new("/h/b"))
        );
        a.save(&[record(1)]).unwrap();
        b.save(&[record(2), record(3)]).unwrap();
        // b ending its approvals does not touch a's.
        b.save(&[]).unwrap();
        assert_eq!(a.load().unwrap().len(), 1);
        assert!(b.load().unwrap().is_empty());
        a.clear().unwrap();
        assert!(a.load().unwrap().is_empty());
        assert_eq!(backend.snapshot().len(), 1);
    }

    #[test]
    fn the_generation_only_goes_up_even_for_a_new_process() {
        let backend = Arc::new(InMemorySecretBackend::default());
        let gen = |backend: &InMemorySecretBackend| -> u64 {
            let text = backend.get(&account("h")).unwrap().unwrap();
            serde_json::from_str::<SavedApprovals>(&text)
                .unwrap()
                .generation
        };
        let first = RecordsInKeychain::new(backend.clone(), "h");
        first.save(&[]).unwrap();
        first.save(&[]).unwrap();
        assert_eq!(gen(&backend), 2);
        // A restarted daemon starts from what is stored.
        let second = RecordsInKeychain::new(backend.clone(), "h");
        second.load().unwrap();
        second.save(&[]).unwrap();
        assert_eq!(gen(&backend), 3);
        // Even one that did not load first writes above what it finds.
        let third = RecordsInKeychain::new(backend.clone(), "h");
        third.save(&[]).unwrap();
        assert_eq!(gen(&backend), 4);
    }

    #[test]
    fn the_group_is_matched_exactly() {
        let groups = vec!["F4M5J7YNLS.app.nodespace.daemon".to_string()];
        assert!(groups_include(&groups, ACCESS_GROUP));
        for other in [
            "F4M5J7YNLS.app.nodespace",
            "F4M5J7YNLS.app.nodespace.daemon.x",
            "F4M5J7YNLS.*",
            "",
        ] {
            assert!(!groups_include(&groups, other), "{other}");
        }
        assert!(!groups_include(&[], ACCESS_GROUP));
        assert!(!groups_include(&["F4M5J7YNLS.*".to_string()], ACCESS_GROUP));
    }
}
