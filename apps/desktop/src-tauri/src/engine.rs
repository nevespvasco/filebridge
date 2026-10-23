//! Phones (ADR 0014): the engine runs in the app's own process, with librclone linked in. Same
//! interface as the desktop's [`crate::service`] (`client()`, `endpoint()`, the same events), so
//! the commands generated from the ipc catalogue don't change: they call
//! `host::EngineOperations` instead of the socket client.
//!
//! While a Job is busy (analysing, sending, retrying, waiting, verifying, updating the tool),
//! Android's foreground service keeps the process alive and shows the progress.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};

use filebridge_host::{Access, EngineOperations, Host, HostOptions, TransportChoice};
use filebridge_ipc::{ErrorBody, Event, Handler, JobState};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_specta::Event as _;
use tokio::sync::{OnceCell, broadcast};

use crate::native::{Native, SendNotice};

/// Every engine event, as on desktop.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct ServiceEvent(pub Event);

/// Always `connected: true` once the engine is open: there is no link to lose on a phone.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct ConnectionChanged {
    pub connected: bool,
    pub endpoint: String,
}

const ENDPOINT: &str = "in-app engine";

/// Shared handle, kept in Tauri's state.
#[derive(Clone)]
pub struct Service(Arc<Inner>);

struct Inner {
    app: AppHandle,
    engine: OnceCell<Opened>,
    labels: StdMutex<NoticeLabels>,
}

struct Opened {
    _host: Host,
    ops: Arc<EngineOperations>,
}

/// The foreground notification's text, translated by the UI (`set_send_notice_labels`).
#[derive(Clone)]
struct NoticeLabels {
    title: String,
    one: String,
    many: String,
}

impl Default for NoticeLabels {
    fn default() -> Self {
        // Portuguese until the UI sends its translations, like the desktop tray.
        Self {
            title: "filebridge".into(),
            one: "A enviar 1 transferência".into(),
            many: "A enviar {n} transferências".into(),
        }
    }
}

fn unavailable(message: String) -> ErrorBody {
    ErrorBody {
        code: "service_unavailable".into(),
        message,
    }
}

impl Service {
    pub fn new(app: AppHandle) -> Self {
        Self(Arc::new(Inner {
            app,
            engine: OnceCell::new(),
            labels: StdMutex::new(NoticeLabels::default()),
        }))
    }

    pub fn endpoint(&self) -> String {
        ENDPOINT.into()
    }

    /// Never: the engine lives in the app, so there is no older one to replace.
    pub fn replacing(&self) -> bool {
        false
    }

    /// The engine's operations, opening the engine first (once).
    #[allow(clippy::type_complexity)]
    pub fn client(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<EngineOperations>, ErrorBody>> + Send + '_>> {
        Box::pin(async move {
            let opened = self.0.engine.get_or_try_init(|| self.open()).await?;
            Ok(opened.ops.clone())
        })
    }

    pub fn set_notice_labels(&self, title: String, one: String, many: String) {
        *self.0.labels.lock().unwrap() = NoticeLabels { title, one, many };
    }

    async fn open(&self) -> Result<Opened, ErrorBody> {
        let app = &self.0.app;
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|e| unavailable(format!("no data folder: {e}")))?;
        // Android gives a process no HOME: rclone looks for its config and cache there, and
        // SSH profiles for `~/.ssh/known_hosts`. Both belong in the app's own folders.
        let cache_dir = app
            .path()
            .app_cache_dir()
            .unwrap_or_else(|_| data_dir.join("cache"));
        for dir in [&data_dir, &cache_dir] {
            std::fs::create_dir_all(dir)
                .map_err(|e| unavailable(format!("create {}: {e}", dir.display())))?;
        }
        // SAFETY: set once, before librclone and the engine start any thread that reads the
        // environment.
        unsafe {
            std::env::set_var("HOME", &data_dir);
            std::env::set_var("TMPDIR", &cache_dir);
        }
        let native = app.state::<Native>().inner().clone();
        let host = Host::open_with_secrets(
            &HostOptions {
                data_dir: Some(data_dir),
                rclone: None,
                transport: TransportChoice::Librclone,
            },
            Access::Exclusive,
            native.secret_store(),
        )
        .await
        .map_err(|e| unavailable(format!("{e:#}")))?;
        let ops = Arc::new(host.operations());
        tauri::async_runtime::spawn(self.clone().forward_events(ops.subscribe(), native));
        // Jobs a closed app left mid-way go on now, as the desktop service does on start.
        match host.engine.resume_unfinished().await {
            Ok(resumed) if !resumed.is_empty() => {
                tracing::info!("resumed {} interrupted jobs", resumed.len());
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("could not resume interrupted jobs: {e}"),
        }
        if let Err(e) = (ConnectionChanged {
            connected: true,
            endpoint: ENDPOINT.into(),
        })
        .emit(app)
        {
            tracing::warn!("could not emit the connection state: {e}");
        }
        Ok(Opened { _host: host, ops })
    }

    /// Forwards every event to the window, and keeps the foreground service in step with the
    /// busy Jobs.
    async fn forward_events(self, mut rx: broadcast::Receiver<Event>, native: Native) {
        let app = &self.0.app;
        let mut busy: HashMap<String, Option<u8>> = HashMap::new();
        let mut shown: Option<SendNotice> = None;
        loop {
            let event = match rx.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("the window missed {n} progress events");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            };
            match &event {
                Event::JobStateChanged { job_id, to, .. } => {
                    if keeps_awake(*to) {
                        busy.entry(job_id.clone()).or_insert(None);
                    } else {
                        busy.remove(job_id);
                    }
                }
                Event::TransferProgress {
                    job_id,
                    bytes_done,
                    bytes_total,
                    ..
                } => {
                    if let Some(p) = busy.get_mut(job_id) {
                        *p = (*bytes_total > 0)
                            .then(|| (bytes_done.saturating_mul(100) / bytes_total).min(100) as u8);
                    }
                }
                _ => {}
            }
            if let Err(e) = ServiceEvent(event).emit(app) {
                tracing::warn!("could not forward an event: {e}");
            }
            let notice = self.notice(&busy);
            if notice != shown {
                native.sending(notice.as_ref()).await;
                shown = notice;
            }
        }
    }

    /// What the notification says for these busy Jobs; `None` stops the foreground service.
    fn notice(&self, busy: &HashMap<String, Option<u8>>) -> Option<SendNotice> {
        if busy.is_empty() {
            return None;
        }
        let labels = self.0.labels.lock().unwrap().clone();
        let text = if busy.len() == 1 {
            labels.one
        } else {
            labels.many.replace("{n}", &busy.len().to_string())
        };
        // One Job: its share sent. Several: unknown (an indeterminate bar).
        let progress = match busy.values().collect::<Vec<_>>().as_slice() {
            [Some(p)] => i32::from(*p),
            _ => -1,
        };
        Some(SendNotice {
            title: labels.title,
            text,
            progress,
        })
    }
}

/// States in which the engine is working on a Job, or waiting to (a retry, the send window),
/// and so needs the process to stay alive.
fn keeps_awake(state: JobState) -> bool {
    matches!(
        state,
        JobState::Analysing
            | JobState::Transferring
            | JobState::Retrying
            | JobState::Waiting
            | JobState::Verifying
            | JobState::Integrating
    )
}
