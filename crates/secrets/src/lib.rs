//! Secrets in the OS keyring (ADR 0004): a profile's app password and its SSH key. SQLite, the
//! config, logs and error messages only ever hold a [`KeyringRef`], never the secret.
//!
//! [`KeyringStore`] is the real one: Keychain on macOS, Credential Manager on Windows, Secret
//! Service on Linux (GNOME Keyring, KWallet). There is no file fallback: without a keyring the
//! call fails with an error that says how to get one. [`MemoryStore`] is for tests and the
//! service's fake-transport mode. Calls block (a D-Bus or system round trip): async callers wrap
//! them in `spawn_blocking`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

pub use filebridge_transport_api::KeyringRef;
pub use zeroize::Zeroizing;

/// The keyring service every filebridge secret is stored under.
pub const SERVICE: &str = "filebridge";

/// What a profile's secret is for; part of its reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    /// A password: Nextcloud app password (WebDAV), or an SFTP password.
    AppPassword,
    /// The private key filebridge generated for the profile (OpenSSH format).
    SshKey,
}

impl SecretKind {
    pub const ALL: [SecretKind; 2] = [SecretKind::AppPassword, SecretKind::SshKey];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppPassword => "app-password",
            Self::SshKey => "ssh-key",
        }
    }
}

/// `filebridge/<profile-id>/<kind>`: where a profile's secret of this kind lives.
pub fn reference(profile_id: &str, kind: SecretKind) -> KeyringRef {
    KeyringRef(format!("{SERVICE}/{profile_id}/{}", kind.as_str()))
}

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// Nothing stored under this reference (never the value; the reference is not secret).
    #[error("no secret stored for {0}")]
    NotFound(String),
    /// No keyring to use: no Secret Service running, a locked keychain the user refused, …
    #[error(
        "the system keyring is not available ({0}). On Linux, filebridge needs a Secret Service \
         such as GNOME Keyring or KWallet, unlocked in your session"
    )]
    Unavailable(String),
    #[error("keyring: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SecretError>;

/// A value whose `Debug` and `Display` never show it, for secrets that pass through structs
/// that get logged.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Redacted<T>(pub T);

impl<T> fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl<T> fmt::Display for Redacted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// A secret read from the store: wiped from memory when dropped, never printed.
pub type Secret = Redacted<Zeroizing<String>>;

/// Where secrets are kept.
pub trait SecretStore: Send + Sync {
    fn get(&self, r: &KeyringRef) -> Result<Secret>;
    fn set(&self, r: &KeyringRef, value: &str) -> Result<()>;
    /// Removes the secret; one that isn't there is not an error.
    fn delete(&self, r: &KeyringRef) -> Result<()>;
}

/// The OS keyring.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringStore;

impl KeyringStore {
    fn entry(r: &KeyringRef) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, &r.0).map_err(convert)
    }
}

/// keyring's errors, without anything that could carry the secret.
fn convert(e: keyring::Error) -> SecretError {
    use keyring::Error as E;
    match e {
        E::PlatformFailure(e) | E::NoStorageAccess(e) => SecretError::Unavailable(e.to_string()),
        E::NoEntry => SecretError::NotFound(String::new()),
        other => SecretError::Other(other.to_string()),
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, r: &KeyringRef) -> Result<Secret> {
        match Self::entry(r)?.get_password() {
            Ok(v) => Ok(Redacted(Zeroizing::new(v))),
            Err(keyring::Error::NoEntry) => Err(SecretError::NotFound(r.0.clone())),
            Err(e) => Err(convert(e)),
        }
    }

    fn set(&self, r: &KeyringRef, value: &str) -> Result<()> {
        Self::entry(r)?.set_password(value).map_err(convert)
    }

    fn delete(&self, r: &KeyringRef) -> Result<()> {
        match Self::entry(r)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(convert(e)),
        }
    }
}

/// In memory, for tests and the fake-transport service. Lost on exit.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, Zeroizing<String>>>);

impl SecretStore for MemoryStore {
    fn get(&self, r: &KeyringRef) -> Result<Secret> {
        let map = self.0.lock().expect("secret map lock");
        map.get(&r.0)
            .map(|v| Redacted(v.clone()))
            .ok_or_else(|| SecretError::NotFound(r.0.clone()))
    }

    fn set(&self, r: &KeyringRef, value: &str) -> Result<()> {
        let mut map = self.0.lock().expect("secret map lock");
        map.insert(r.0.clone(), Zeroizing::new(value.to_owned()));
        Ok(())
    }

    fn delete(&self, r: &KeyringRef) -> Result<()> {
        self.0.lock().expect("secret map lock").remove(&r.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_name_the_profile_and_the_kind() {
        assert_eq!(
            reference("p-1", SecretKind::AppPassword).0,
            "filebridge/p-1/app-password"
        );
        assert_eq!(
            reference("p-1", SecretKind::SshKey).0,
            "filebridge/p-1/ssh-key"
        );
    }

    #[test]
    fn a_secret_never_prints() {
        let s: Secret = Redacted(Zeroizing::new("hunter2".into()));
        assert_eq!(format!("{s:?} {s}"), "*** ***");
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Holder {
            pass: Secret,
        }
        assert!(!format!("{:?}", Holder { pass: s }).contains("hunter2"));
    }

    #[test]
    fn memory_store_round_trip() {
        let store = MemoryStore::default();
        let r = reference("p-1", SecretKind::AppPassword);
        assert!(matches!(store.get(&r), Err(SecretError::NotFound(_))));
        store.set(&r, "app-pass").unwrap();
        assert_eq!(store.get(&r).unwrap().0.as_str(), "app-pass");
        store.delete(&r).unwrap();
        store.delete(&r).unwrap(); // twice is fine
        assert!(store.get(&r).is_err());
    }

    #[test]
    fn errors_never_carry_the_value() {
        let store = MemoryStore::default();
        let r = reference("p-1", SecretKind::SshKey);
        let msg = store.get(&r).unwrap_err().to_string();
        assert_eq!(msg, "no secret stored for filebridge/p-1/ssh-key");
    }

    /// Touches the real keyring of whoever runs it: `cargo test -p filebridge-secrets -- --ignored`.
    #[test]
    #[ignore]
    fn the_os_keyring_round_trip() {
        let store = KeyringStore;
        let r = reference("test-only", SecretKind::AppPassword);
        store.set(&r, "value-for-a-test").unwrap();
        assert_eq!(store.get(&r).unwrap().0.as_str(), "value-for-a-test");
        store.delete(&r).unwrap();
        assert!(matches!(store.get(&r), Err(SecretError::NotFound(_))));
    }
}
