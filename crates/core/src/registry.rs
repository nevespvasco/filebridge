//! The plugins this build knows. Filled once at startup by the binary that hosts the engine
//! (`apps/service`, `apps/cli`): a new plugin is a new crate plus one `add_*` line there.

use std::collections::HashMap;
use std::sync::Arc;

use filebridge_integration_api::Integration;
use filebridge_names::{ActiveFor, NamePlan, NameRule, RuleSet, general_rules};
use filebridge_transport_api::Transport;

use crate::{CoreError, Profile, Result};

#[derive(Default)]
pub struct Registry {
    transports: HashMap<String, Arc<dyn Transport>>,
    integrations: HashMap<String, Arc<dyn Integration>>,
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

    pub(crate) fn transports(&self) -> impl Iterator<Item = &Arc<dyn Transport>> {
        self.transports.values()
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

    /// Rejects a profile whose transport or integration is unknown, whose integration needs
    /// capabilities the transport doesn't have, or whose settings its integration refuses
    /// (checked here, not at send time).
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
            integration
                .check_profile(&profile.destination, &profile.integration_settings)
                .map_err(|e| CoreError::Invalid(format!("{id}: {}", e.message)))?;
        }
        Ok(())
    }

    /// The Name plan for jobs to `profile`: the general rules (Windows ones on Windows targets)
    /// plus the rules its integration imposes for its settings.
    pub fn name_plan(&self, profile: &Profile) -> Result<NamePlan> {
        let mut rules: Vec<Box<dyn NameRule>> = general_rules(ActiveFor::windows());
        if let Some(id) = &profile.integration {
            rules.extend(
                self.integration(id)?
                    .name_rules(&profile.integration_settings),
            );
        }
        let target = profile.target_profile();
        Ok(NamePlan::new(
            RuleSet::new(rules, &target),
            target.case_sensitive,
        ))
    }
}
