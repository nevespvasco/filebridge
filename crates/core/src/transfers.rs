//! The Transfers list: names, search and groups, and Send again with the earlier job's name
//! decisions (docs/design/specs/D06-transfers-list.md).

use filebridge_model::{JobFilter, JobPage, JobSort, Op};
use filebridge_store::{JobOrder, JobQuery, NewJob};

use crate::{CoreError, Engine, JobKind, JobState, Result, decisions};

/// The longest job name, in characters.
pub const MAX_LABEL: usize = 120;

impl Engine {
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
    /// profile. Its analysis reuses the earlier job's name decisions. A repair is checked again
    /// on its own destination only: the folder it repaired is there, so another profile is
    /// refused.
    pub async fn send_again(&self, job_id: &str, profile_id: Option<String>) -> Result<String> {
        let _job = self.begin(job_id, Op::SendAgain).await?;
        let row = self.store.get_job(job_id).await?;
        if JobKind::parse(&row.kind)? == JobKind::Repair {
            if profile_id.is_some_and(|p| p != row.profile_id) {
                return Err(CoreError::Invalid(
                    "a repair checks a folder of its own destination: it can only be run again \
                     there"
                        .into(),
                ));
            }
            // Again = check the same folder again: names there may have changed since.
            let options: crate::SendOptions =
                serde_json::from_value(row.options).unwrap_or_default();
            return self
                .create_repair_job(&row.profile_id, options.target_dir)
                .await;
        }
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
                schedule_id: None,
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
        let kept = copied - self.recheck_carried(job_id).await?;
        self.log(
            Some(job_id),
            "decisions",
            serde_json::json!({ "carried": true, "count": kept, "again_of": from }),
        )
        .await
    }

    /// Puts back to pending the carried-over decisions that this job's rules refuse. Returns
    /// how many.
    pub(crate) async fn recheck_carried(&self, job_id: &str) -> Result<u64> {
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
            let issue = decisions::decided(r);
            if let Some(choice) = &issue.choice
                && names.check(entry, &issue, choice).is_err()
            {
                refused.push(id);
            }
        }
        let n = refused.len() as u64;
        if !refused.is_empty() {
            self.store.reset_issues(job_id, refused).await?;
        }
        Ok(n)
    }
}
