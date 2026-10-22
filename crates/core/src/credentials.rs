//! A profile's password, kept in the OS keyring (ADR 0004): the profile holds only its
//! reference. Set from the window or the CLI, read by the transport when it connects.

use filebridge_secrets::{SecretKind, reference};
use filebridge_transport_api::Backend;

use crate::{CoreError, Engine, Profile, Result};

impl Engine {
    /// Stores `password` for the profile in the keyring and points the profile at it, or, with
    /// `None`, removes both. Only destinations that sign in with a password have one.
    pub async fn set_profile_password(
        &self,
        id: &str,
        password: Option<String>,
    ) -> Result<Profile> {
        let mut profile = self.get_profile(id).await?;
        let slot = match &mut profile.destination.backend {
            Backend::Sftp { password, .. } | Backend::Webdav { password, .. } => password,
            Backend::Local => {
                return Err(CoreError::Invalid("a local folder has no password".into()));
            }
        };
        let secrets = self.registry.secrets()?;
        let r = reference(id, SecretKind::AppPassword);
        let key = r.clone();
        let stored = match password.filter(|p| !p.is_empty()) {
            Some(value) => {
                let res = tokio::task::spawn_blocking(move || secrets.set(&key, &value)).await;
                *slot = Some(r);
                res
            }
            None => {
                let res = tokio::task::spawn_blocking(move || secrets.delete(&key)).await;
                *slot = None;
                res
            }
        };
        stored
            .map_err(|e| CoreError::Invalid(format!("keyring task failed: {e}")))?
            .map_err(|e| CoreError::Invalid(e.to_string()))?;
        self.store
            .upsert_profile(crate::profile::to_row(&profile)?)
            .await?;
        Ok(profile)
    }
}
