//! App-wide settings (docs/adr/0010): kept in the store, applied to every transport, and watched
//! by running jobs so a new bandwidth limit or send window takes effect at once.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use chrono::Timelike;
use filebridge_model::{AppSettings, WindowStatus};
use filebridge_store::now_ms;
use tokio_util::sync::CancellationToken;

use crate::{CoreError, Engine, Event, Result};

const SETTINGS_KEY: &str = "app";
/// The longest a wait on the window sleeps before looking at the clock again: the monotonic
/// clock stops while the computer sleeps, and wall-clock time can jump (DST, a clock change).
const RECHECK: Duration = Duration::from_secs(60);

/// Seconds since local midnight: the system's local time, or a fixed function in tests.
#[derive(Clone, Default)]
pub struct LocalClock(Option<Arc<dyn Fn() -> u32 + Send + Sync>>);

impl LocalClock {
    pub fn fixed(f: impl Fn() -> u32 + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(f)))
    }

    pub fn second_of_day(&self) -> u32 {
        match &self.0 {
            Some(f) => f(),
            None => chrono::Local::now().time().num_seconds_from_midnight(),
        }
    }
}

impl fmt::Debug for LocalClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "LocalClock(fixed)"
        } else {
            "LocalClock(system)"
        })
    }
}

impl Engine {
    /// Reads the saved settings and applies the bandwidth limit. Called once when the host opens
    /// the engine; a transport that can't take the limit is logged, not fatal.
    pub async fn load_settings(&self) -> Result<AppSettings> {
        let settings: AppSettings = self
            .store
            .get_setting_json(SETTINGS_KEY)
            .await?
            .unwrap_or_default();
        if let Err(e) = self.apply_bandwidth_limit(settings.bandwidth_limit).await {
            tracing::warn!("cannot apply the bandwidth limit: {e}");
        }
        self.settings.send_replace(settings.clone());
        Ok(settings)
    }

    pub fn get_settings(&self) -> AppSettings {
        self.settings.borrow().clone()
    }

    /// Validates, saves and applies the settings. Running and waiting jobs pick up a new send
    /// window at once.
    pub async fn save_settings(&self, settings: AppSettings) -> Result<()> {
        settings.validate().map_err(CoreError::Invalid)?;
        self.store.set_setting_json(SETTINGS_KEY, &settings).await?;
        let limit = settings.bandwidth_limit;
        self.settings.send_replace(settings);
        self.apply_bandwidth_limit(limit).await
    }

    async fn apply_bandwidth_limit(&self, limit: Option<u64>) -> Result<()> {
        for transport in self.registry.transports() {
            transport.set_bandwidth_limit(limit).await?;
        }
        Ok(())
    }

    /// The send window now, or `None` when files may be sent at any time.
    fn window_status(&self) -> Option<WindowStatus> {
        let window = self.settings.borrow().send_window?;
        Some(window.status(self.options.local_clock.second_of_day()))
    }

    pub(crate) fn window_closed(&self) -> bool {
        self.window_status().is_some_and(|s| !s.open)
    }

    /// Returns once the send window is closed. Never returns while there is no window.
    pub(crate) async fn window_closes(&self) {
        let mut rx = self.settings.subscribe();
        loop {
            let wait = match self.window_status() {
                Some(s) if !s.open => return,
                Some(s) => Duration::from_secs(s.change_in_secs.into()).min(RECHECK),
                None => RECHECK,
            };
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = rx.changed() => {}
            }
        }
    }

    /// Waits until the send window is open, telling the UI when it opens. Returns `false` if the
    /// job was cancelled (or paused) meanwhile.
    pub(crate) async fn window_opens(&self, job_id: &str, cancel: &CancellationToken) -> bool {
        let mut rx = self.settings.subscribe();
        let mut told: Option<i64> = None;
        loop {
            let status = match self.window_status() {
                Some(s) if !s.open => s,
                _ => return true,
            };
            let opens_at = now_ms() + i64::from(status.change_in_secs) * 1000;
            // A new window, or the clock moving more than a minute against the estimate.
            if told.is_none_or(|t| (t - opens_at).abs() > RECHECK.as_millis() as i64) {
                told = Some(opens_at);
                self.emit(Event::SendWindowWait {
                    job_id: job_id.to_owned(),
                    opens_at,
                });
            }
            let wait = Duration::from_secs(status.change_in_secs.into()).min(RECHECK);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = rx.changed() => {}
                _ = cancel.cancelled() => return false,
            }
        }
    }
}
