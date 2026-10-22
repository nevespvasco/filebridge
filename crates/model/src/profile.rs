//! Destination profiles: where a job sends to, through which transport and integration.

use filebridge_names::{TargetFamily, TargetProfile};
use filebridge_transport_api::Destination;
use serde::{Deserialize, Serialize};

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
    /// Longest name the destination takes, in UTF-8 bytes. Default 255.
    #[serde(default)]
    pub max_name_bytes: Option<u32>,
    /// Longest path under the destination's root, in UTF-8 bytes. Default 260 on Windows (its
    /// classic limit), 4096 elsewhere.
    #[serde(default)]
    pub max_path_bytes: Option<u32>,
}

impl Default for TargetSpec {
    fn default() -> Self {
        Self {
            family: Family::Unix,
            case_sensitive: true,
            max_name_bytes: None,
            max_path_bytes: None,
        }
    }
}

/// Windows' classic `MAX_PATH`, the limit most of its tools still keep to.
const WINDOWS_MAX_PATH: usize = 260;

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

impl Profile {
    /// What the name rules need to know about the destination.
    pub fn target_profile(&self) -> TargetProfile {
        let defaults = TargetProfile::default();
        let family = match self.target.family {
            Family::Unix => TargetFamily::Unix,
            Family::Windows => TargetFamily::Windows,
            Family::Macos => TargetFamily::MacOs,
        };
        let max_path = match family {
            TargetFamily::Windows => WINDOWS_MAX_PATH,
            _ => defaults.max_path_bytes,
        };
        TargetProfile {
            family,
            case_sensitive: self.target.case_sensitive,
            integration: self.integration.clone(),
            max_name_bytes: self
                .target
                .max_name_bytes
                .map_or(defaults.max_name_bytes, |n| n as usize),
            max_path_bytes: self.target.max_path_bytes.map_or(max_path, |n| n as usize),
        }
    }
}
