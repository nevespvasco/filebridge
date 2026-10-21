//! The plugins this build knows. Filled once at startup by the binary that hosts the engine
//! (`apps/service`, `apps/cli`): a new plugin is a new crate plus one `add_*` line there.

use std::collections::HashMap;
use std::sync::Arc;

use filebridge_integration_api::Integration;
use filebridge_names::{ActiveFor, NameRule, RuleSet, general_rules};
use filebridge_transport_api::Transport;

use crate::{CoreError, Profile, Result};

#[derive(Default)]
pub struct Registry {
    transports: HashMap<String, Arc<dyn Transport>>,
    integrations: HashMap<String, Arc<dyn Integration>>,
    /// Integrations that need Windows-compatible names (forbidden characters, trailing dots).
    windows_compat: Vec<String>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_transport(&mut self, transport: Arc<dyn Transport>) -> &mut Self {
        self.transports.insert(transport.id().to_owned(), transport);
        self
    }

    pub fn add_integration(&mut self, integration: Arc<dyn Integration>) -> &mut Self {
        self.integrations
            .insert(integration.id().to_owned(), integration);
        self
    }

    /// Applies the Windows-compatibility rules to targets of this integration too.
    pub fn require_windows_names_for(&mut self, integration_id: &str) -> &mut Self {
        self.windows_compat.push(integration_id.to_owned());
        self
    }

    pub fn transport(&self, id: &str) -> Result<Arc<dyn Transport>> {
        self.transports
            .get(id)
            .cloned()
            .ok_or_else(|| CoreError::Invalid(format!("unknown transport {id:?}")))
    }

    pub fn integration(&self, id: &str) -> Result<Arc<dyn Integration>> {
        self.integrations
            .get(id)
            .cloned()
            .ok_or_else(|| CoreError::Invalid(format!("unknown integration {id:?}")))
    }

    pub fn transport_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.transports.keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn integration_ids(&self) -> Vec<String> {
        let mut ids: Vec<_> = self.integrations.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Rejects a profile whose transport or integration is unknown, or whose integration needs
    /// capabilities the transport doesn't have (checked here, not at send time).
    pub fn check_profile(&self, profile: &Profile) -> Result<()> {
        let transport = self.transport(&profile.destination.transport)?;
        if let Some(id) = &profile.integration {
            let integration = self.integration(id)?;
            let missing = integration.required_caps() - transport.capabilities();
            if !missing.is_empty() {
                return Err(CoreError::Invalid(format!(
                    "integration {id:?} needs {missing:?}, which transport {:?} lacks",
                    transport.id()
                )));
            }
        }
        Ok(())
    }

    /// The name rules for jobs to `profile`: the general rules plus the integration's own.
    pub fn rule_set(&self, profile: &Profile) -> Result<RuleSet> {
        let compat = ActiveFor::windows().and_integrations(self.windows_compat.iter().cloned());
        let mut rules: Vec<Box<dyn NameRule>> = general_rules(compat);
        if let Some(id) = &profile.integration {
            rules.extend(self.integration(id)?.name_rules());
        }
        Ok(RuleSet::new(rules, &profile.target_profile()))
    }
}
