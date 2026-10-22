//! A destination's space: measured by the preflights, kept per profile, read without a network
//! call (docs/design/specs/D03-destination-storage.md).

use filebridge_model::{STORAGE_STALE_AFTER_MS, SpaceView, StorageView};
use filebridge_store::now_ms;

use crate::{Engine, PreflightView, Profile, ProfileTest, Result};

impl Engine {
    /// Saves a connection test as the profile's latest; best effort (a profile deleted meanwhile
    /// is fine).
    pub(crate) async fn keep_test(&self, profile_id: &str, test: &ProfileTest) {
        let ok = !test.transport.has_failures();
        if let Ok(json) = serde_json::to_value(test) {
            let _ = self.store.save_profile_test(profile_id, json, ok).await;
        }
    }

    /// Runs both preflights for `profile` and keeps the result.
    pub(crate) async fn measure(&self, profile: &Profile, job_id: &str) -> Result<ProfileTest> {
        let mut it = self.preflight(profile, job_id).await?.into_iter();
        let test = ProfileTest {
            transport: it.next().expect("transport preflight is always present"),
            integration: it.next(),
        };
        self.keep_test(&profile.id, &test).await;
        Ok(test)
    }

    /// The space of a destination from its last check; `refresh` checks it first (network).
    pub async fn profile_storage(&self, id: &str, refresh: bool) -> Result<StorageView> {
        if refresh {
            let profile = self.get_profile(id).await?;
            self.measure(&profile, "profile-storage").await?;
        }
        let stored = self.store.profile_test(id).await?;
        let last_error = stored.last.as_ref().and_then(|(v, _)| {
            let t: ProfileTest = serde_json::from_value(v.clone()).ok()?;
            t.transport
                .checks
                .iter()
                .find(|c| c.status == "fail")
                .map(|c| c.message.clone())
        });
        let good = stored
            .last_good
            .and_then(|(v, at)| Some((serde_json::from_value::<ProfileTest>(v).ok()?, at)));
        let Some((test, at)) = good else {
            return Ok(StorageView {
                profile_id: id.to_owned(),
                free_bytes: None,
                total_bytes: None,
                used_bytes: None,
                transport: None,
                quota: None,
                checked_at: None,
                stale: true,
                low: false,
                last_error,
            });
        };
        let transport = space(&test.transport);
        let quota = test
            .integration
            .as_ref()
            .map(space)
            .filter(|q| q.free_bytes.is_some());
        // The tighter of the two sides: the one with less free space.
        let effective = match (transport.free_bytes, quota.and_then(|q| q.free_bytes)) {
            (Some(t), Some(q)) if q < t => quota.unwrap_or_default(),
            (None, Some(_)) => quota.unwrap_or_default(),
            _ => transport,
        };
        Ok(StorageView {
            profile_id: id.to_owned(),
            free_bytes: effective.free_bytes,
            total_bytes: effective.total_bytes,
            used_bytes: effective.used_bytes,
            transport: Some(transport)
                .filter(|t| t.free_bytes.is_some() || t.total_bytes.is_some()),
            quota: quota.filter(|q| q != &transport),
            checked_at: Some(at),
            stale: now_ms() - at > STORAGE_STALE_AFTER_MS,
            low: StorageView::is_low(effective.free_bytes, effective.total_bytes),
            last_error,
        })
    }
}

fn space(p: &PreflightView) -> SpaceView {
    SpaceView {
        free_bytes: p.free_bytes,
        total_bytes: p.total_bytes,
        used_bytes: p.used_bytes,
    }
}
