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
//! Platform limits: a value is at most [`MAX_SECRET_VALUE_BYTES`] bytes
//! everywhere, and on Windows also at most 2560 bytes as the credential manager
//! stores it (UTF-16, so 1280 UTF-16 units). The names index is one item, so
//! on Windows it has the same 2560-byte ceiling: roughly 19 names of full
//! length, or about 70 of 16 characters. A value or a new name that would pass
//! the platform limit is refused with an error saying so, and nothing is left
//! behind.
//!
//! The operating system store cannot be enumerated, so the store keeps an index
//! of names (names only) as one more item beside the values. Values and the
//! index sit under reserved account prefixes, so no name can collide with the
//! index.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use zeroize::{Zeroize, Zeroizing};

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
    /// A value (or other item) that the platform's credential store cannot hold.
    #[error("{what} too long for this platform: the credential store accepts at most {max} bytes per item as stored")]
    TooLong { what: &'static str, max: usize },
    /// The names index would pass the platform's item limit.
    #[error("too many secrets for this platform: the list of names is limited to {max} bytes as stored; remove a secret first")]
    TooManySecrets { max: usize },
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

impl SecretValue {
    /// Overwrites the value in place.
    fn wipe(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.wipe();
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
    /// The largest item this store holds, measured by [`item_size`](Self::item_size);
    /// `None` when it has no limit below [`MAX_SECRET_VALUE_BYTES`].
    fn max_item_size(&self) -> Option<usize> {
        None
    }
    /// How large `text` is as this store keeps it.
    fn item_size(&self, text: &str) -> usize {
        text.len()
    }
}

/// The Windows credential manager's item ceiling, in bytes as stored.
pub const WINDOWS_MAX_ITEM_BYTES: usize = 2560;

/// A string's size as the Windows credential manager stores it: UTF-16.
pub fn utf16_item_size(text: &str) -> usize {
    text.encode_utf16().count() * 2
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
        if let Some(max) = self.backend.max_item_size() {
            if self.backend.item_size(&text) > max {
                return Err(SecretError::TooManySecrets { max });
            }
        }
        self.backend.set(INDEX_ACCOUNT, &text)
    }

    /// Stores `value` under `name`, replacing any earlier value.
    pub fn set(&self, name: &str, value: &str) -> Result<(), SecretError> {
        validate_secret_name(name)?;
        validate_secret_value(value)?;
        if let Some(max) = self.backend.max_item_size() {
            if self.backend.item_size(value) > max {
                return Err(SecretError::TooLong { what: "value", max });
            }
        }
        let _guard = self.index_lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut names = self.read_index()?;
        let account = Self::value_account(name);
        // What the account held, kept so a failure puts it back: an unlisted
        // value may already be there, and it must not be lost to a failed set.
        let old = self.backend.get(&account)?.map(Zeroizing::new);
        self.backend.set(&account, value)?;
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
            names.sort();
            if let Err(e) = self.write_index(&names) {
                // Leave the account as it was: the old value, or nothing.
                let _ = match &old {
                    Some(previous) => self.backend.set(&account, previous),
                    None => self.backend.delete(&account).map(|_| ()),
                };
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
    if let keyring::Error::TooLong(_, max) = error {
        return SecretError::TooLong {
            what: "item",
            max: max as usize,
        };
    }
    let kind = match error {
        keyring::Error::PlatformFailure(_) => "the platform store failed",
        keyring::Error::NoStorageAccess(_) => "the platform store is locked or refused access",
        keyring::Error::Invalid(..) => "an item is not accepted by the platform store",
        keyring::Error::Ambiguous(_) => "several items match",
        _ => "the platform store reported an error",
    };
    SecretError::Store(kind.to_string())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl SecretBackend for KeychainBackend {
    fn max_item_size(&self) -> Option<usize> {
        cfg!(windows).then_some(WINDOWS_MAX_ITEM_BYTES)
    }
    fn item_size(&self, text: &str) -> usize {
        if cfg!(windows) {
            utf16_item_size(text)
        } else {
            text.len()
        }
    }
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

    /// Fails the keychain arm over to env? It must not: an empty store with an
    /// environment that answers every name still reports the name as not set.
    #[test]
    fn a_keychain_reference_never_falls_back_to_the_environment() {
        let (s, _) = store();
        let env = |_: &str| Some("from-env".to_string());
        let err =
            resolve_credential_with(&s, &CredentialRef::Keychain("X".into()), env).unwrap_err();
        assert_eq!(err, SecretError::NotSet("X".into()));
    }

    /// A backend with the Windows ceiling, to test the limit logic anywhere.
    struct WindowsLike(InMemorySecretBackend);
    impl SecretBackend for WindowsLike {
        fn set(&self, account: &str, value: &str) -> Result<(), SecretError> {
            assert!(
                utf16_item_size(value) <= WINDOWS_MAX_ITEM_BYTES,
                "the store was asked to hold an item over its ceiling"
            );
            self.0.set(account, value)
        }
        fn get(&self, account: &str) -> Result<Option<String>, SecretError> {
            self.0.get(account)
        }
        fn delete(&self, account: &str) -> Result<bool, SecretError> {
            self.0.delete(account)
        }
        fn max_item_size(&self) -> Option<usize> {
            Some(WINDOWS_MAX_ITEM_BYTES)
        }
        fn item_size(&self, text: &str) -> usize {
            utf16_item_size(text)
        }
    }

    fn windows_like() -> (SecretStore, Arc<WindowsLike>) {
        let backend = Arc::new(WindowsLike(InMemorySecretBackend::default()));
        (SecretStore::new(backend.clone()), backend)
    }

    #[test]
    fn a_value_over_the_platform_ceiling_is_refused_with_a_clear_error_and_nothing_is_written() {
        let (s, backend) = windows_like();
        // 1280 UTF-16 units fit; 1281 do not.
        assert!(s.set("a", &"x".repeat(1280)).is_ok());
        let err = s.set("b", &"x".repeat(1281)).unwrap_err();
        assert_eq!(
            err,
            SecretError::TooLong {
                what: "value",
                max: WINDOWS_MAX_ITEM_BYTES
            }
        );
        assert!(err.to_string().contains("too long for this platform"));
        assert!(err.to_string().contains("2560"));
        assert!(!backend.0.snapshot().contains_key("value:b"));
        assert_eq!(s.names().unwrap(), vec!["a".to_string()]);
    }

    #[test]
    fn the_ceiling_counts_utf16_units_not_bytes() {
        let (s, _) = windows_like();
        // A character outside the basic plane is two UTF-16 units.
        assert!(s.set("a", &"\u{1F600}".repeat(640)).is_ok());
        assert!(s.set("b", &"\u{1F600}".repeat(641)).is_err());
        // Three bytes in UTF-8, one unit in UTF-16: 1280 of them fit.
        assert!(s.set("c", &"\u{20AC}".repeat(1280)).is_ok());
    }

    #[test]
    fn a_name_that_would_overflow_the_names_item_is_refused_and_its_value_rolled_back() {
        let (s, backend) = windows_like();
        let mut stored = 0;
        let mut refused = None;
        for i in 0..200 {
            let name = format!("{:0>64}", i);
            match s.set(&name, "v") {
                Ok(()) => stored += 1,
                Err(e) => {
                    refused = Some((name, e));
                    break;
                }
            }
        }
        let (name, err) = refused.expect("the index must run out");
        assert!(stored >= 15, "{stored}");
        assert_eq!(
            err,
            SecretError::TooManySecrets {
                max: WINDOWS_MAX_ITEM_BYTES
            }
        );
        assert!(err
            .to_string()
            .contains("too many secrets for this platform"));
        // The refused name left no value and no entry; the others are intact.
        assert!(!backend.0.snapshot().contains_key(&format!("value:{name}")));
        assert_eq!(s.names().unwrap().len(), stored);
        assert!(!s.names().unwrap().contains(&name));
        // Deleting one makes room again.
        let first = s.names().unwrap()[0].clone();
        s.delete(&first).unwrap();
        assert!(s.set(&name, "v").is_ok());
    }

    /// A value already stored but unlisted (an orphan) must survive a failed
    /// `set`: the old value is put back, not deleted.
    #[test]
    fn a_failed_set_restores_an_orphaned_old_value() {
        let backend = Arc::new(FailingIndex(InMemorySecretBackend::default()));
        backend.0.set("value:a", "OLD").unwrap();
        let s = SecretStore::new(backend.clone());
        assert!(s.set("a", "NEW").is_err());
        assert_eq!(backend.0.get("value:a").unwrap(), Some("OLD".to_string()));
    }

    #[test]
    fn a_resolved_value_is_zeroed_when_dropped() {
        let mut v = SecretValue::new("ghp_SECRET".to_string());
        let ptr = v.0.as_ptr();
        let len = v.0.len();
        // What `Drop` runs, called while the buffer is still allocated so the
        // bytes can be read afterwards.
        v.wipe();
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        assert!(
            bytes.iter().all(|b| *b == 0),
            "the value was not overwritten"
        );
        std::mem::forget(v);
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
