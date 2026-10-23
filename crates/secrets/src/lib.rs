//! Secrets in the OS keyring (ADR 0004): a profile's app password, its SSH key and a tool's API
//! key. SQLite, the config, logs and error messages only ever hold a [`KeyringRef`], never the
//! secret.
//!
//! [`KeyringStore`] is the real one: Keychain on macOS and iOS, Credential Manager on Windows,
//! Secret Service on Linux (GNOME Keyring, KWallet). There is no file fallback: without a keyring
//! the call fails with an error that says how to get one. On Android it always fails: the app
//! passes its own Keystore-backed store to the host instead (ADR 0014). [`MemoryStore`] is for
//! tests and the service's fake-transport mode. Calls block (a D-Bus or system round trip, maybe
//! an unlock prompt): async callers go through [`blocking`], which runs them off the async
//! threads and gives up after [`TIMEOUT`].
//!
//! Plugins never get a [`SecretStore`]: the host hands them a [`StoreResolver`], which only reads,
//! through the `transport-api` contract ([`SecretResolver`]).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
pub use filebridge_transport_api::KeyringRef;
use filebridge_transport_api::{ErrorKind, SecretResolver, SecretValue, TransportError};
pub use zeroize::Zeroizing;

/// The keyring service every filebridge secret is stored under.
pub const SERVICE: &str = "filebridge";

/// What a profile's secret is for; part of its reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    /// The secret the user types for the destination: a Nextcloud app password (WebDAV), an
    /// SFTP or SMB password, or an S3 secret access key (ADR 0022).
    AppPassword,
    /// The private key filebridge generated for the profile (OpenSSH format).
    SshKey,
    /// A tool's API key, e.g. Immich's.
    ApiKey,
}

impl SecretKind {
    pub const ALL: [SecretKind; 3] = [
        SecretKind::AppPassword,
        SecretKind::SshKey,
        SecretKind::ApiKey,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppPassword => "app-password",
            Self::SshKey => "ssh-key",
            Self::ApiKey => "api-key",
        }
    }
}

/// `filebridge/<profile-id>/<kind>`: where a profile's secret of this kind lives.
pub fn reference(profile_id: &str, kind: SecretKind) -> KeyringRef {
    KeyringRef(format!("{SERVICE}/{profile_id}/{}", kind.as_str()))
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SecretError {
    /// Nothing stored under this reference (never the value; the reference is not secret).
    #[error("no secret stored for {0}")]
    NotFound(String),
    /// No keyring to use: no Secret Service running, a locked keychain the user refused, a
    /// keyring that doesn't answer, … Trying again later (once it is unlocked) may work.
    #[error(
        "the system keyring is not available ({0}). On Linux, filebridge needs a Secret Service \
         such as GNOME Keyring or KWallet, unlocked in your session"
    )]
    Unavailable(String),
    #[error("keyring: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, SecretError>;

impl From<SecretError> for TransportError {
    /// Missing is `Auth` (set it again); an unreachable keyring is `Transient` (unlock it and
    /// the send goes on), never `Auth`: the password may be fine.
    fn from(e: SecretError) -> Self {
        let kind = match e {
            SecretError::NotFound(_) => ErrorKind::Auth,
            SecretError::Unavailable(_) => ErrorKind::Transient,
            SecretError::Other(_) => ErrorKind::Other,
        };
        TransportError::new(kind, e.to_string())
    }
}

/// How long a keyring call may take before [`blocking`] gives up. Long enough to type a password
/// into an unlock prompt; a keyring that never answers (a hung D-Bus service) doesn't hang the
/// caller.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// Runs `f` on `store` off the async threads, giving up after [`TIMEOUT`] with
/// [`SecretError::Unavailable`]. The call itself can't be interrupted: it finishes, unseen, on
/// its blocking thread.
pub async fn blocking<T: Send + 'static>(
    store: &Arc<dyn SecretStore>,
    f: impl FnOnce(&dyn SecretStore) -> Result<T> + Send + 'static,
) -> Result<T> {
    blocking_within(TIMEOUT, store, f).await
}

async fn blocking_within<T: Send + 'static>(
    limit: Duration,
    store: &Arc<dyn SecretStore>,
    f: impl FnOnce(&dyn SecretStore) -> Result<T> + Send + 'static,
) -> Result<T> {
    let store = store.clone();
    let task = tokio::task::spawn_blocking(move || f(store.as_ref()));
    match tokio::time::timeout(limit, task).await {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => Err(SecretError::Other(format!("keyring task failed: {e}"))),
        Err(_) => Err(SecretError::Unavailable(format!(
            "it did not answer within {} s; is it locked?",
            limit.as_secs()
        ))),
    }
}

/// The read-only side of a [`SecretStore`] that plugins get (ADR 0004, docs/03).
pub struct StoreResolver(pub Arc<dyn SecretStore>);

#[async_trait]
impl SecretResolver for StoreResolver {
    async fn resolve(&self, r: &KeyringRef) -> filebridge_transport_api::Result<SecretValue> {
        let r = r.clone();
        let secret = blocking(&self.0, move |s| s.get(&r)).await?;
        Ok(SecretValue::new(secret.0.as_str().to_owned()))
    }
}

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

/// keyring's errors about `r`, without anything that could carry the secret.
#[cfg(not(target_os = "android"))]
fn convert(r: &KeyringRef, e: keyring::Error) -> SecretError {
    use keyring::Error as E;
    match e {
        E::PlatformFailure(e) | E::NoStorageAccess(e) => SecretError::Unavailable(e.to_string()),
        E::NoEntry => SecretError::NotFound(r.0.clone()),
        other => SecretError::Other(other.to_string()),
    }
}

#[cfg(not(target_os = "android"))]
impl KeyringStore {
    fn entry(r: &KeyringRef) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, &r.0).map_err(|e| convert(r, e))
    }
}

#[cfg(not(target_os = "android"))]
impl SecretStore for KeyringStore {
    fn get(&self, r: &KeyringRef) -> Result<Secret> {
        Self::entry(r)?
            .get_password()
            .map(|v| Redacted(Zeroizing::new(v)))
            .map_err(|e| convert(r, e))
    }

    fn set(&self, r: &KeyringRef, value: &str) -> Result<()> {
        Self::entry(r)?
            .set_password(value)
            .map_err(|e| convert(r, e))
    }

    fn delete(&self, r: &KeyringRef) -> Result<()> {
        match Self::entry(r)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(convert(r, e)),
        }
    }
}

/// Android: no keyring here; the app's Keystore store is used instead.
#[cfg(target_os = "android")]
impl SecretStore for KeyringStore {
    fn get(&self, _: &KeyringRef) -> Result<Secret> {
        Err(android_unavailable())
    }

    fn set(&self, _: &KeyringRef, _: &str) -> Result<()> {
        Err(android_unavailable())
    }

    fn delete(&self, _: &KeyringRef) -> Result<()> {
        Err(android_unavailable())
    }
}

#[cfg(target_os = "android")]
fn android_unavailable() -> SecretError {
    SecretError::Other("no keyring on Android outside the filebridge app".into())
}

/// In memory, for tests and the fake-transport service. Lost on exit.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, Zeroizing<String>>>);

impl MemoryStore {
    /// Nothing stored: what a test checks after a wizard that gave up.
    pub fn is_empty(&self) -> bool {
        self.0.lock().expect("secret map lock").is_empty()
    }
}

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
        assert_eq!(
            reference("p-1", SecretKind::ApiKey).0,
            "filebridge/p-1/api-key"
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

    #[test]
    fn a_missing_entry_names_its_reference() {
        let r = reference("p-1", SecretKind::AppPassword);
        let e = convert(&r, keyring::Error::NoEntry);
        assert_eq!(
            e.to_string(),
            "no secret stored for filebridge/p-1/app-password"
        );
    }

    #[test]
    fn an_unreachable_keyring_is_transient_not_auth() {
        let e: TransportError = SecretError::Unavailable("locked".into()).into();
        assert_eq!(e.kind, ErrorKind::Transient);
        assert!(e.message.contains("Secret Service"), "{}", e.message);
        let e: TransportError = SecretError::NotFound("filebridge/p/ssh-key".into()).into();
        assert_eq!(e.kind, ErrorKind::Auth);
    }

    /// A keyring that never answers (a hung Secret Service, a prompt nobody sees).
    struct Hung;
    impl SecretStore for Hung {
        fn get(&self, _: &KeyringRef) -> Result<Secret> {
            std::thread::sleep(Duration::from_millis(300));
            Err(SecretError::Other("too late".into()))
        }
        fn set(&self, _: &KeyringRef, _: &str) -> Result<()> {
            Ok(())
        }
        fn delete(&self, _: &KeyringRef) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_keyring_that_does_not_answer_times_out_as_unavailable() {
        let store: Arc<dyn SecretStore> = Arc::new(Hung);
        let r = reference("p-1", SecretKind::AppPassword);
        let e = blocking_within(Duration::from_millis(20), &store, move |s| s.get(&r))
            .await
            .unwrap_err();
        assert!(matches!(e, SecretError::Unavailable(_)), "{e}");
        assert!(e.to_string().contains("did not answer"), "{e}");
    }

    #[tokio::test]
    async fn the_resolver_reads_and_maps_errors() {
        let store = Arc::new(MemoryStore::default());
        let r = reference("p-1", SecretKind::AppPassword);
        store.set(&r, "app-pass").unwrap();
        let resolver = StoreResolver(store.clone());
        assert_eq!(resolver.resolve(&r).await.unwrap().expose(), "app-pass");
        store.delete(&r).unwrap();
        let e = resolver.resolve(&r).await.unwrap_err();
        assert_eq!(e.kind, ErrorKind::Auth);
        assert!(e.message.contains("filebridge/p-1/app-password"), "{e}");
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
