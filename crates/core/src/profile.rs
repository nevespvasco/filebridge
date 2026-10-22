//! How profiles are stored: the `profiles` row and its JSON config.

use filebridge_store::ProfileRow;
use filebridge_transport_api::Destination;
use serde::{Deserialize, Serialize};

use crate::{CoreError, Profile, Result, TargetSpec};

/// The JSON kept in `profiles.config`.
#[derive(Serialize, Deserialize)]
struct StoredConfig {
    destination: Destination,
    #[serde(default)]
    integration_settings: serde_json::Value,
    #[serde(default)]
    target: TargetSpec,
}

pub(crate) fn to_row(profile: &Profile) -> Result<ProfileRow> {
    if profile.name.trim().is_empty() {
        return Err(CoreError::Invalid("a profile needs a name".into()));
    }
    let config = serde_json::to_value(StoredConfig {
        destination: profile.destination.clone(),
        integration_settings: profile.integration_settings.clone(),
        target: profile.target.clone(),
    })?;
    Ok(ProfileRow {
        id: profile.id.clone(),
        name: profile.name.clone(),
        transport: profile.destination.transport.clone(),
        integration: profile.integration.clone(),
        config,
        created_at: 0,
        updated_at: 0,
    })
}

pub(crate) fn from_row(row: ProfileRow) -> Result<Profile> {
    let config: StoredConfig = serde_json::from_value(row.config)?;
    Ok(Profile {
        id: row.id,
        name: row.name,
        destination: config.destination,
        integration: row.integration,
        integration_settings: config.integration_settings,
        target: config.target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use filebridge_transport_api::Backend;

    #[test]
    fn row_round_trip() {
        let p = Profile {
            id: "p1".into(),
            name: "Nextcloud casa".into(),
            destination: Destination {
                transport: "rclone".into(),
                backend: Backend::Local,
                root: "/srv/data".into(),
            },
            integration: Some("nextcloud".into()),
            integration_settings: serde_json::json!({ "mode": "direct" }),
            target: TargetSpec::default(),
        };
        let row = to_row(&p).unwrap();
        assert_eq!(row.transport, "rclone");
        assert_eq!(from_row(row).unwrap(), p);
        assert_eq!(p.target_profile().integration.as_deref(), Some("nextcloud"));
    }
}
