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

/// The longest name and path limits a profile may set.
const MAX_NAME_LIMIT: u32 = 4096;
const MAX_PATH_LIMIT: u32 = 65_536;

/// Refuses name and path limits the Name plan can't work with: a name limit below the shortest
/// name it proposes, a path limit shorter than a name, or absurdly large ones.
fn check_limits(target: &TargetSpec) -> Result<()> {
    let min = filebridge_names::MIN_NAME_LEN as u32;
    if let Some(n) = target.max_name_bytes
        && !(min..=MAX_NAME_LIMIT).contains(&n)
    {
        return Err(CoreError::Invalid(format!(
            "the longest name must be between {min} and {MAX_NAME_LIMIT}"
        )));
    }
    if let Some(p) = target.max_path_bytes {
        let floor = target.max_name_bytes.unwrap_or(min).max(min);
        if !(floor..=MAX_PATH_LIMIT).contains(&p) {
            return Err(CoreError::Invalid(format!(
                "the longest path must be between {floor} and {MAX_PATH_LIMIT}, and no \
                 shorter than the longest name"
            )));
        }
    }
    Ok(())
}

pub(crate) fn to_row(profile: &Profile) -> Result<ProfileRow> {
    if profile.name.trim().is_empty() {
        return Err(CoreError::Invalid("a profile needs a name".into()));
    }
    check_limits(&profile.target)?;
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
    fn name_and_path_limits_are_checked() {
        let spec = |name, path| TargetSpec {
            max_name_bytes: name,
            max_path_bytes: path,
            ..TargetSpec::default()
        };
        assert!(check_limits(&spec(None, None)).is_ok());
        assert!(check_limits(&spec(Some(255), Some(260))).is_ok());
        assert!(check_limits(&spec(Some(0), None)).is_err(), "no name fits");
        assert!(
            check_limits(&spec(Some(8), None)).is_err(),
            "below the shortest cut"
        );
        assert!(
            check_limits(&spec(Some(255), Some(100))).is_err(),
            "path < name"
        );
        assert!(check_limits(&spec(None, Some(10))).is_err());
        assert!(check_limits(&spec(Some(1 << 20), None)).is_err());
    }

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
