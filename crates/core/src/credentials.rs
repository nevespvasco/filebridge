//! A profile's password, kept in the OS keyring (ADR 0004): the profile holds only its
//! reference. Set from the window or the CLI, read by the transport when it connects.

use filebridge_secrets::{SecretKind, reference};

use crate::{CoreError, Engine, Profile, Result};

impl Engine {
    /// Stores `password` for the profile in the keyring and points the profile at it, or, with
    /// `None`, removes both. Only destinations that sign in with a password have one, and a
    /// profile that needs one (Nextcloud's WebDAV mode) can't lose it: the changed profile is
    /// checked before the keyring is touched.
    pub async fn set_profile_password(
        &self,
        id: &str,
        password: Option<String>,
    ) -> Result<Profile> {
        let mut profile = self.get_profile(id).await?;
        let password = password.filter(|p| !p.is_empty());
        let backend = &mut profile.destination.backend;
        let kind = backend.kind();
        let Some(slot) = backend.password_mut() else {
            return Err(CoreError::Invalid(format!(
                "a {kind} destination has no password"
            )));
        };
        let r = reference(id, SecretKind::AppPassword);
        *slot = password.is_some().then(|| r.clone());
        self.registry.check_profile(&profile)?;
        let secrets = self.registry.secrets()?;
        match password {
            Some(value) => filebridge_secrets::blocking(&secrets, move |s| s.set(&r, &value)).await,
            None => filebridge_secrets::blocking(&secrets, move |s| s.delete(&r)).await,
        }
        .map_err(|e| CoreError::Invalid(e.to_string()))?;
        self.store
            .upsert_profile(crate::profile::to_row(&profile)?)
            .await?;
        Ok(profile)
    }
}
