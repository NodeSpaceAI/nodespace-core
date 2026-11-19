//! The device's store for the credentials that remote tools refer to.
//!
//! A tool node never holds a credential: it holds a reference, `keychain:<name>`
//! or `env:<NAME>` ([`CredentialRef`]). This module keeps the values behind the
//! `keychain:` references in the operating system's credential store and
//! resolves a reference to its value at the moment a call needs it.
//!
//! The rules this module enforces:
//!
//! - A value is accepted by [`SecretStore::set`] and returned only by
//!   [`SecretStore::get`] / [`resolve_credential`], as a [`SecretValue`]. That
//!   type has no `Display`, no `Serialize`, and a `Debug` that prints nothing
//!   of the value.
//! - Names can be listed ([`SecretStore::names`]); values never can.
//! - No error carries a value: every [`SecretError`] names the reference or
//!   the rule, nothing else.
//! - Names follow the grammar of a `keychain:` reference, so a name that is
//!   stored is a name a node can refer to.
//!
//! The operating system store cannot be enumerated, so the store keeps an index
//! of names (names only) as one more item beside the values. Values and the
//! index sit under reserved account prefixes, so no name can collide with the
//! index.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

pub use nodespace_types::MAX_SECRET_VALUE_BYTES;
use nodespace_types::{is_secret_name, secret_value_problem, CredentialRef};

/// Service name the operating system store files every item under.
pub const KEYCHAIN_SERVICE: &str = "nodespace.secrets";

/// Account prefix of a value item; the name follows.
const VALUE_PREFIX: &str = "value:";
/// Account of the names index. Not a valid value account: it has no prefix.
const INDEX_ACCOUNT: &str = "index";

/// A failure of the secret store. No variant carries a credential value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// The name is not a well-formed `keychain:` name.
    #[error("invalid secret name: use 1 to 64 characters from letters, digits, `.`, `_` and `-`")]
    InvalidName,
    /// The value breaks a rule (the reason names the rule, never the value).
    #[error("invalid secret value: {0}")]
    InvalidValue(&'static str),
    /// A `keychain:` reference whose name has no value on this device.
    #[error("credential `keychain:{0}` is not set on this device; run `nodespace secret set {0}`")]
    NotSet(String),
    /// An `env:` reference whose variable is absent or empty in this process.
    #[error("credential `env:{0}` is not set in the environment of this process")]
    EnvNotSet(String),
    /// The credential store failed or is not available on this platform.
    #[error("credential store: {0}")]
    Store(String),
}

/// A resolved credential value. Deliberately hard to leak: no `Display`, no
/// `Serialize`, no `Clone`, and `Debug` prints a placeholder.
pub struct SecretValue(String);

impl SecretValue {
    /// Wraps a value. Callers that accept values from outside validate first.
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// The value. The one way to read it; call it only where it is injected
    /// into a request or an environment.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<redacted>)")
    }
}

/// Checks a secret name: the grammar of a `keychain:` reference.
pub fn validate_secret_name(name: &str) -> Result<(), SecretError> {
    if is_secret_name(name) {
        Ok(())
    } else {
        Err(SecretError::InvalidName)
    }
}

/// Checks a value before it is stored.
pub fn validate_secret_value(value: &str) -> Result<(), SecretError> {
    match secret_value_problem(value) {
        Some(reason) => Err(SecretError::InvalidValue(reason)),
        None => Ok(()),
    }
}

/// Raw item storage under an account string. The operating system store
/// implements it; tests use [`InMemorySecretBackend`].
pub trait SecretBackend: Send + Sync {
    fn set(&self, account: &str, value: &str) -> Result<(), SecretError>;
    /// `None` is "no such item", which is not an error.
    fn get(&self, account: &str) -> Result<Option<String>, SecretError>;
    /// Removing an absent item succeeds. Returns whether an item was removed.
    fn delete(&self, account: &str) -> Result<bool, SecretError>;
}

/// Names and values on top of a [`SecretBackend`], with the names index.
pub struct SecretStore {
    backend: Arc<dyn SecretBackend>,
    /// Serialises index updates within this process (one daemon owns the store).
    index_lock: Mutex<()>,
}

impl SecretStore {
    pub fn new(backend: Arc<dyn SecretBackend>) -> Self {
        Self {
            backend,
            index_lock: Mutex::new(()),
        }
    }

    /// The operating system's credential store.
    pub fn keychain() -> Self {
        Self::new(Arc::new(KeychainBackend))
    }

    fn value_account(name: &str) -> String {
        format!("{VALUE_PREFIX}{name}")
    }

    fn read_index(&self) -> Result<Vec<String>, SecretError> {
        match self.backend.get(INDEX_ACCOUNT)? {
            None => Ok(Vec::new()),
            Some(text) => serde_json::from_str::<Vec<String>>(&text)
                .map_err(|_| SecretError::Store("the names index is unreadable".to_string())),
        }
    }

    fn write_index(&self, names: &[String]) -> Result<(), SecretError> {
        let text = serde_json::to_string(names)
            .map_err(|_| SecretError::Store("the names index cannot be written".to_string()))?;
        self.backend.set(INDEX_ACCOUNT, &text)
    }

    /// Stores `value` under `name`, replacing any earlier value.
    pub fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        validate_secret_name(name)?;
        validate_secret_value(value)?;
        let _guard = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut names = self.read_index()?;
        let account = Self::value_account(name);
        self.backend.set(&account, value)?;
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
            names.sort();
            if let Err(e) = self.write_index(&names) {
                // Do not leave a value that nothing lists.
                let _ = self.backend.delete(&account);
                return Err(e);
            }
        }
        Ok(())
    }

    /// The value under `name`, if one is set.
    pub fn get(&self, name: &str) -> Result<Option<SecretValue>, SecretError> {
        validate_secret_name(name)?;
        Ok(self
            .backend
            .get(&Self::value_account(name))?
            .map(SecretValue::new))
    }

    /// Removes `name`. Returns whether a value was there. Removing an unset
    /// name succeeds.
    pub fn delete(&self, name: &str) -> Result<bool, SecretError> {
        validate_secret_name(name)?;
        let _guard = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        let existed = self.backend.delete(&Self::value_account(name))?;
        let mut names = self.read_index()?;
        let before = names.len();
        names.retain(|n| n != name);
        if names.len() != before {
            self.write_index(&names)?;
        }
        Ok(existed)
    }

    /// The names that have a value on this device, sorted. Never values.
    pub fn names(&self) -> Result<Vec<String>, SecretError> {
        let _guard = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.read_index()
    }
}

/// Resolves a credential reference to its value, at call time.
///
/// `env_lookup` reads the process environment; an empty value counts as not
/// set. Errors name the reference only.
pub fn resolve_credential_with(
    store: &SecretStore,
    reference: &CredentialRef,
    env_lookup: impl Fn(&str) -> Option<String>,
) -> Result<SecretValue, SecretError> {
    match reference {
        CredentialRef::Keychain(name) => store
            .get(name)?
            .ok_or_else(|| SecretError::NotSet(name.clone())),
        CredentialRef::Env(name) => match env_lookup(name) {
            Some(value) if !value.is_empty() => Ok(SecretValue::new(value)),
            _ => Err(SecretError::EnvNotSet(name.clone())),
        },
    }
}

/// [`resolve_credential_with`] against this process's environment.
pub fn resolve_credential(
    store: &SecretStore,
    reference: &CredentialRef,
) -> Result<SecretValue, SecretError> {
    resolve_credential_with(store, reference, |name| std::env::var(name).ok())
}

/// In-memory backend for tests of this module and of code that uses the store;
/// it never touches the operating system's store.
#[derive(Default)]
pub struct InMemorySecretBackend {
    items: Mutex<HashMap<String, String>>,
}

impl InMemorySecretBackend {
    /// Every stored (account, value) pair, for tests that assert what was
    /// written where.
    pub fn snapshot(&self) -> HashMap<String, String> {
        self.items.lock().unwrap().clone()
    }
}

impl SecretBackend for InMemorySecretBackend {
    fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
        self.items
            .lock()
            .unwrap()
            .insert(account.to_string(), value.to_string());
        Ok(())
    }
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        Ok(self.items.lock().unwrap().get(account).cloned())
    }
    fn delete(&self, account: &str) -> Result<bool, SecretError> {
        Ok(self.items.lock().unwrap().remove(account).is_some())
    }
}

/// The operating system's credential store (macOS keychain, Windows Credential
/// Manager). On other platforms every call reports the store as unavailable and
/// the `env:` reference is the way to supply a credential.
pub struct KeychainBackend;

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl KeychainBackend {
    fn entry(account: &str) -> Result<keyring::Entry, SecretError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(store_error)
    }
}

/// Maps a keyring failure to a message that cannot carry a value: only the
/// failure's kind is kept.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn store_error(error: keyring::Error) -> SecretError {
    let kind = match error {
        keyring::Error::PlatformFailure(_) => "the platform store failed",
        keyring::Error::NoStorageAccess(_) => "the platform store is locked or refused access",
        keyring::Error::TooLong(..) => "an item is too long for the platform store",
        keyring::Error::Invalid(..) => "an item is not accepted by the platform store",
        keyring::Error::Ambiguous(_) => "several items match",
        _ => "the platform store reported an error",
    };
    SecretError::Store(kind.to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl SecretBackend for KeychainBackend {
    fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
        Self::entry(account)?
            .set_password(value)
            .map_err(store_error)
    }
    fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
        match Self::entry(account)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(store_error(e)),
        }
    }
    fn delete(&self, account: &str) -> Result<bool, SecretError> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(e) => Err(store_error(e)),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl SecretBackend for KeychainBackend {
    fn set(&self, _account: &str, _value: &str) -> Result<(), SecretError> {
        Err(unavailable())
    }
    fn get(&self, _account: &str) -> Result<Option<String>, SecretError> {
        Err(unavailable())
    }
    fn delete(&self, _account: &str) -> Result<bool, SecretError> {
        Err(unavailable())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn unavailable() -> SecretError {
    SecretError::Store(
        "the operating system credential store is not available on this platform; use an `env:` reference"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (SecretStore, Arc<InMemorySecretBackend>) {
        let backend = Arc::new(InMemorySecretBackend::default());
        (SecretStore::new(backend.clone()), backend)
    }

    const TOKEN: &str = "ghp_SUPERSECRETVALUE123";

    #[test]
    fn a_stored_value_is_read_back_and_listed_by_name_only() {
        let (s, _) = store();
        s.set("github-token", TOKEN).unwrap();
        assert_eq!(s.get("github-token").unwrap().unwrap().expose(), TOKEN);
        assert_eq!(s.names().unwrap(), vec!["github-token".to_string()]);
    }

    #[test]
    fn the_names_listing_never_contains_a_value() {
        let (s, backend) = store();
        s.set("a", TOKEN).unwrap();
        let listing = format!("{:?}", s.names().unwrap());
        assert!(!listing.contains(TOKEN));
        // The index item holds names only.
        let index = backend.snapshot().get(INDEX_ACCOUNT).cloned().unwrap();
        assert!(!index.contains(TOKEN));
    }

    #[test]
    fn names_are_sorted_and_not_duplicated_on_overwrite() {
        let (s, _) = store();
        s.set("b", "1").unwrap();
        s.set("a", "2").unwrap();
        s.set("b", "3").unwrap();
        assert_eq!(s.names().unwrap(), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(s.get("b").unwrap().unwrap().expose(), "3");
    }

    #[test]
    fn delete_removes_value_and_name_and_is_idempotent() {
        let (s, backend) = store();
        s.set("a", TOKEN).unwrap();
        assert!(s.delete("a").unwrap());
        assert!(s.get("a").unwrap().is_none());
        assert!(s.names().unwrap().is_empty());
        assert!(!s.delete("a").unwrap());
        assert!(!backend.snapshot().values().any(|v| v == TOKEN));
    }

    #[test]
    fn a_name_that_is_not_a_keychain_name_is_refused_everywhere() {
        let (s, _) = store();
        for bad in [
            "",
            "has space",
            "a/b",
            "x:y",
            &"n".repeat(65),
            "ünï",
            "a\nb",
        ] {
            assert_eq!(s.set(bad, "v"), Err(SecretError::InvalidName), "{bad:?}");
            assert!(matches!(s.get(bad), Err(SecretError::InvalidName)));
            assert!(matches!(s.delete(bad), Err(SecretError::InvalidName)));
        }
        assert!(s.set(&"n".repeat(64), "v").is_ok());
    }

    #[test]
    fn a_bad_value_is_refused_and_nothing_is_written() {
        let (s, backend) = store();
        let long = "x".repeat(MAX_SECRET_VALUE_BYTES + 1);
        for bad in ["", "a\0b", long.as_str()] {
            assert!(matches!(s.set("a", bad), Err(SecretError::InvalidValue(_))));
        }
        assert!(backend.snapshot().is_empty());
        assert!(s.set("a", &"x".repeat(MAX_SECRET_VALUE_BYTES)).is_ok());
    }

    #[test]
    fn no_error_message_or_debug_output_carries_the_value() {
        let (s, _) = store();
        let errors = [
            s.set("bad name", TOKEN).unwrap_err(),
            SecretError::NotSet("a".into()),
            SecretError::EnvNotSet("A".into()),
            SecretError::Store("failed".into()),
        ];
        for e in errors {
            assert!(!e.to_string().contains(TOKEN));
            assert!(!format!("{e:?}").contains(TOKEN));
        }
        let v = SecretValue::new(TOKEN.to_string());
        assert!(!format!("{v:?}").contains(TOKEN));
        assert!(!format!("{:#?}", Some(&v)).contains(TOKEN));
    }

    #[test]
    fn a_name_cannot_reach_the_index_item() {
        let (s, _) = store();
        // `index` is a valid name; its value lives under the value prefix, so
        // it cannot overwrite the index.
        s.set("index", TOKEN).unwrap();
        s.set("other", "1").unwrap();
        assert_eq!(
            s.names().unwrap(),
            vec!["index".to_string(), "other".to_string()]
        );
        assert_eq!(s.get("index").unwrap().unwrap().expose(), TOKEN);
    }

    #[test]
    fn keychain_reference_resolves_and_reports_an_unset_name_without_a_value() {
        let (s, _) = store();
        s.set("tok", TOKEN).unwrap();
        let ok =
            resolve_credential_with(&s, &CredentialRef::Keychain("tok".into()), |_| None).unwrap();
        assert_eq!(ok.expose(), TOKEN);
        let missing =
            resolve_credential_with(&s, &CredentialRef::Keychain("none".into()), |_| None)
                .unwrap_err();
        assert_eq!(missing, SecretError::NotSet("none".into()));
        assert!(missing.to_string().contains("nodespace secret set none"));
    }

    #[test]
    fn env_reference_reads_the_environment_and_treats_empty_as_unset() {
        let (s, _) = store();
        let env = |name: &str| match name {
            "HAS" => Some(TOKEN.to_string()),
            "EMPTY" => Some(String::new()),
            _ => None,
        };
        let ok = resolve_credential_with(&s, &CredentialRef::Env("HAS".into()), env).unwrap();
        assert_eq!(ok.expose(), TOKEN);
        for name in ["EMPTY", "ABSENT"] {
            assert_eq!(
                resolve_credential_with(&s, &CredentialRef::Env(name.into()), env).unwrap_err(),
                SecretError::EnvNotSet(name.into())
            );
        }
    }

    #[test]
    fn an_env_reference_never_reads_the_store_and_a_keychain_reference_never_reads_env() {
        let (s, _) = store();
        s.set("X", "from-store").unwrap();
        let env = |_: &str| Some("from-env".to_string());
        assert_eq!(
            resolve_credential_with(&s, &CredentialRef::Env("X".into()), env)
                .unwrap()
                .expose(),
            "from-env"
        );
        assert_eq!(
            resolve_credential_with(&s, &CredentialRef::Keychain("X".into()), env)
                .unwrap()
                .expose(),
            "from-store"
        );
    }

    /// A backend whose index write fails: the value must not be left behind.
    struct FailingIndex(InMemorySecretBackend);
    impl SecretBackend for FailingIndex {
        fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
            if account == INDEX_ACCOUNT {
                return Err(SecretError::Store("index write failed".into()));
            }
            self.0.set(account, value)
        }
        fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
            self.0.get(account)
        }
        fn delete(&self, account: &str) -> Result<bool, SecretError> {
            self.0.delete(account)
        }
    }

    #[test]
    fn a_failed_index_write_leaves_no_unlisted_value() {
        let backend = Arc::new(FailingIndex(InMemorySecretBackend::default()));
        let s = SecretStore::new(backend.clone());
        assert!(s.set("a", TOKEN).is_err());
        assert!(backend.0.snapshot().is_empty());
    }

    #[test]
    fn an_unreadable_index_is_an_error_not_an_empty_list() {
        let (s, backend) = store();
        backend.set(INDEX_ACCOUNT, "not json").unwrap();
        assert!(matches!(s.names(), Err(SecretError::Store(_))));
        // And a set does not overwrite the unreadable index with a one-name list.
        assert!(s.set("a", "v").is_err());
        assert_eq!(backend.snapshot().get(INDEX_ACCOUNT).unwrap(), "not json");
    }
}
