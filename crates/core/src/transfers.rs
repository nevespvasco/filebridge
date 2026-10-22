//! The Transfers list: names, search and groups, and Send again with the earlier job's name
//! decisions (docs/design/specs/D06-transfers-list.md).

use filebridge_model::{JobFilter, JobPage, JobSort, Op};
use filebridge_store::{JobOrder, JobQuery, NewJob};

use crate::{CoreError, Engine, JobKind, JobState, Result, decisions};

/// The longest job name, in characters.
pub const MAX_LABEL: usize = 120;

impl Engine {
    /// Jobs whose sources can be freed safely (docs/design/specs/D10). Nothing is, until the
    /// source re-check of D10 exists: it is one of the conditions.
    pub(crate) async fn reclaimable_ids(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// A page of jobs matching `filter`, and how many match in all.
    pub async fn list_jobs_page(
        &self,
        filter: JobFilter,
        limit: u32,
        offset: u32,
    ) -> Result<JobPage> {
        let ids = if filter.reclaimable {
            Some(self.reclaimable_ids().await?)
        } else {
            None
        };
        let query = JobQuery {
            states: filter
                .groups
                .iter()
                .flat_map(|g| g.states())
                .map(|s| s.as_str().to_owned())
                .collect(),
            profile_id: filter.profile_id,
            text: filter.query.map(|q| q.trim().to_owned()),
            ids,
            order: match filter.sort {
                JobSort::Newest => JobOrder::Newest,
                JobSort::Oldest => JobOrder::Oldest,
                JobSort::Largest => JobOrder::Largest,
            },
        };
        let (rows, total) = self.store.list_jobs_filtered(query, limit, offset).await?;
        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            items.push(self.summary(row).await?);
        }
        Ok(JobPage { items, total })
    }

    /// Names a job, in any state; a blank name clears it.
    pub async fn set_label(&self, job_id: &str, label: Option<String>) -> Result<()> {
        let _job = self.begin(job_id, Op::SetLabel).await?;
        let label = label.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty());
        if let Some(l) = &label {
            if l.chars().count() > MAX_LABEL {
                return Err(CoreError::Invalid(format!(
                    "a name has at most {MAX_LABEL} characters"
                )));
            }
            if l.contains(['\n', '\r']) {
                return Err(CoreError::Invalid("a name is one line".into()));
            }
        }
        self.store.set_job_label(job_id, label).await?;
        Ok(())
    }

    /// A new draft with a finished job's sources, options and name, to `profile_id` or the same
    /// profile. Its analysis reuses the earlier job's name decisions.
    pub async fn send_again(&self, job_id: &str, profile_id: Option<String>) -> Result<String> {
        let _job = self.begin(job_id, Op::SendAgain).await?;
        let row = self.store.get_job(job_id).await?;
        let profile = self
            .get_profile(profile_id.as_deref().unwrap_or(&row.profile_id))
            .await?;
        self.registry.check_profile(&profile)?;
        let id = uuid::Uuid::new_v4().to_string();
        self.store
            .insert_job(NewJob {
                id: id.clone(),
                kind: JobKind::Transfer.as_str().into(),
                profile_id: profile.id.clone(),
                state: JobState::Draft.as_str().into(),
                options: row.options.clone(),
                sources: row.sources.clone(),
                label: row.label.clone(),
                again_of: Some(job_id.to_owned()),
            })
            .await?;
        let sources: Vec<String> = row
            .sources
            .iter()
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        self.log(
            Some(&id),
            "created",
            serde_json::json!({
                "sources": sources, "profile_id": profile.id, "options": row.options,
                "again_of": job_id,
            }),
        )
        .await?;
        Ok(id)
    }

    /// After a repeated job's analysis: copy the earlier job's decisions onto the same entries
    /// and rules, then put back to pending any that the new destination's rules refuse.
    pub(crate) async fn carry_decisions(&self, job_id: &str, from: &str) -> Result<()> {
        let copied = self.store.copy_decisions(from, job_id).await?;
        if copied == 0 {
            return Ok(());
        }
        let names = self.job_name_plan(job_id).await?;
        let entries = self.entries(job_id).await?;
        let mut refused = Vec::new();
        for r in self.all_issues(job_id, None).await? {
            if !r.carried_over {
                continue;
            }
            let id = r.id;
            let Some(entry) = entries.get(r.issue.entry_idx as usize) else {
                continue;
            };
            let rule = r.issue.rule.clone();
            if let Some(choice) = decisions::decided(r).choice
                && names.check(entry, &rule, &choice).is_err()
            {
                refused.push(id);
            }
        }
        let kept = copied - refused.len() as u64;
        if !refused.is_empty() {
            self.store.reset_issues(job_id, refused).await?;
        }
        self.log(
            Some(job_id),
            "decisions",
            serde_json::json!({ "carried": true, "count": kept, "again_of": from }),
        )
        .await
    }
}
