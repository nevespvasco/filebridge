//! Destination profiles: where a job sends to, through which transport and integration.

use filebridge_names::{TargetFamily, TargetProfile};
use filebridge_store::ProfileRow;
use filebridge_transport_api::Destination;
use serde::{Deserialize, Serialize};

use crate::{CoreError, Result};

/// What the destination filesystem accepts; decides which general name rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Unix,
    Windows,
    Macos,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct TargetSpec {
    pub family: Family,
    pub case_sensitive: bool,
}

impl Default for TargetSpec {
    fn default() -> Self {
        Self {
            family: Family::Unix,
            case_sensitive: true,
        }
    }
}

/// A saved destination. Holds keyring references only, never secrets (ADR 0004).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub destination: Destination,
    /// Integration id (e.g. `"nextcloud"`), if the destination is a tool to update after sends.
    pub integration: Option<String>,
    /// The integration's own settings, as its profile wizard saved them.
    #[serde(default)]
    pub integration_settings: serde_json::Value,
    #[serde(default)]
    pub target: TargetSpec,
}

/// The JSON kept in `profiles.config`.
#[derive(Serialize, Deserialize)]
struct StoredConfig {
    destination: Destination,
    #[serde(default)]
    integration_settings: serde_json::Value,
    #[serde(default)]
    target: TargetSpec,
}

impl Profile {
    pub fn target_profile(&self) -> TargetProfile {
        TargetProfile {
            family: match self.target.family {
                Family::Unix => TargetFamily::Unix,
                Family::Windows => TargetFamily::Windows,
                Family::Macos => TargetFamily::MacOs,
            },
            case_sensitive: self.target.case_sensitive,
            integration: self.integration.clone(),
            ..TargetProfile::default()
        }
    }

    pub(crate) fn to_row(&self) -> Result<ProfileRow> {
        if self.name.trim().is_empty() {
            return Err(CoreError::Invalid("a profile needs a name".into()));
        }
        let config = serde_json::to_value(StoredConfig {
            destination: self.destination.clone(),
            integration_settings: self.integration_settings.clone(),
            target: self.target.clone(),
        })?;
        Ok(ProfileRow {
            id: self.id.clone(),
            name: self.name.clone(),
            transport: self.destination.transport.clone(),
            integration: self.integration.clone(),
            config,
            created_at: 0,
            updated_at: 0,
        })
    }

    pub(crate) fn from_row(row: ProfileRow) -> Result<Self> {
        let config: StoredConfig = serde_json::from_value(row.config)?;
        Ok(Self {
            id: row.id,
            name: row.name,
            destination: config.destination,
            integration: row.integration,
            integration_settings: config.integration_settings,
            target: config.target,
        })
    }
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
        let row = p.to_row().unwrap();
        assert_eq!(row.transport, "rclone");
        assert_eq!(Profile::from_row(row).unwrap(), p);
        assert_eq!(p.target_profile().integration.as_deref(), Some("nextcloud"));
    }
}
