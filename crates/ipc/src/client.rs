//! The client side: one connection, concurrent calls matched by id, and an event stream.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use interprocess::local_socket::tokio::{Stream, prelude::*};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};

use crate::{
    AnalysisSummary, DecisionInput, DetectNextcloudRequest, DetectNextcloudResponse, Endpoint,
    Envelope, ErrorBody, Event, IssueView, JobSummary, Outgoing, Pong, Profile, ProfileTest,
    Request, RuleCount, SendOptions,
};

/// Calls waiting for their answer. `None` once the connection is gone, so a later call fails
/// at once instead of waiting for an answer that can't come.
type Pending =
    Arc<Mutex<Option<HashMap<u64, oneshot::Sender<Result<serde_json::Value, ErrorBody>>>>>>;

fn closed() -> ErrorBody {
    ErrorBody {
        code: "disconnected".into(),
        message: "the connection to filebridge-service was closed".into(),
    }
}

/// A connection to `filebridge-service`. Cheap to clone; clones share the connection.
#[derive(Clone)]
pub struct Client {
    out: mpsc::Sender<Vec<u8>>,
    pending: Pending,
    /// The reader task owns the only strong sender, so event receivers see `Closed` once the
    /// connection is gone.
    events: broadcast::WeakSender<Event>,
    next_id: Arc<AtomicU64>,
    subscribed: Arc<Mutex<bool>>,
}

impl Client {
    pub async fn connect(endpoint: &Endpoint) -> std::io::Result<Self> {
        let conn = Stream::connect(endpoint.name()?).await?;
        let (recv, mut send) = conn.split();
        let (out, mut out_rx) = mpsc::channel::<Vec<u8>>(256);
        tokio::spawn(async move {
            while let Some(line) = out_rx.recv().await {
                if send.write_all(&line).await.is_err() {
                    break;
                }
            }
        });

        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let (ev, _) = broadcast::channel(1024);
        let (p, events) = (pending.clone(), ev.downgrade());
        tokio::spawn(async move {
            let mut lines = BufReader::new(recv).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                match serde_json::from_str::<Outgoing>(&line) {
                    Ok(Outgoing::Response { id, ok, err }) => {
                        let tx = p.lock().await.as_mut().and_then(|m| m.remove(&id));
                        if let Some(tx) = tx {
                            let _ = tx.send(match err {
                                Some(e) => Err(e),
                                None => Ok(ok.unwrap_or(serde_json::Value::Null)),
                            });
                        }
                    }
                    Ok(Outgoing::Event { event }) => {
                        let _ = ev.send(event);
                    }
                    Err(e) => tracing::warn!("bad message from the service: {e}"),
                }
            }
            // Connection gone: fail every waiting call and end the event streams.
            drop(ev);
            for (_, tx) in p.lock().await.take().into_iter().flatten() {
                let _ = tx.send(Err(closed()));
            }
        });

        Ok(Self {
            out,
            pending,
            events,
            next_id: Arc::new(AtomicU64::new(1)),
            subscribed: Arc::default(),
        })
    }

    /// Sends a request and decodes its answer.
    pub async fn call<T: DeserializeOwned>(&self, request: Request) -> Result<T, ErrorBody> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match self.pending.lock().await.as_mut() {
            Some(pending) => pending.insert(id, tx),
            None => return Err(closed()),
        };
        let mut line = serde_json::to_vec(&Envelope { id, request }).map_err(|e| ErrorBody {
            code: "invalid".into(),
            message: e.to_string(),
        })?;
        line.push(b'\n');
        if self.out.send(line).await.is_err() {
            if let Some(pending) = self.pending.lock().await.as_mut() {
                pending.remove(&id);
            }
            return Err(closed());
        }
        let value = rx.await.map_err(|_| closed())??;
        serde_json::from_value(value).map_err(|e| ErrorBody {
            code: "internal".into(),
            message: format!("unexpected answer from the service: {e}"),
        })
    }

    /// Engine events. The first call subscribes this connection. The receiver reports
    /// `Closed` when the connection drops; after that this returns `disconnected`.
    pub async fn events(&self) -> Result<broadcast::Receiver<Event>, ErrorBody> {
        let rx = self.events.upgrade().ok_or_else(closed)?.subscribe();
        let mut subscribed = self.subscribed.lock().await;
        if !*subscribed {
            self.call::<()>(Request::Subscribe).await?;
            *subscribed = true;
        }
        Ok(rx)
    }

    pub async fn ping(&self) -> Result<Pong, ErrorBody> {
        self.call(Request::Ping).await
    }

    pub async fn list_profiles(&self) -> Result<Vec<Profile>, ErrorBody> {
        self.call(Request::ListProfiles).await
    }

    pub async fn save_profile(&self, profile: Profile) -> Result<Profile, ErrorBody> {
        self.call(Request::SaveProfile { profile }).await
    }

    pub async fn delete_profile(&self, id: String) -> Result<(), ErrorBody> {
        self.call(Request::DeleteProfile { id }).await
    }

    pub async fn test_profile(&self, id: String) -> Result<ProfileTest, ErrorBody> {
        self.call(Request::TestProfile { id }).await
    }

    pub async fn detect_nextcloud(
        &self,
        req: DetectNextcloudRequest,
    ) -> Result<DetectNextcloudResponse, ErrorBody> {
        self.call(Request::DetectNextcloud(req)).await
    }

    pub async fn create_job(
        &self,
        sources: Vec<String>,
        profile_id: String,
        options: SendOptions,
    ) -> Result<String, ErrorBody> {
        self.call(Request::CreateJob {
            sources,
            profile_id,
            options,
        })
        .await
    }

    pub async fn set_options(&self, job_id: String, options: SendOptions) -> Result<(), ErrorBody> {
        self.call(Request::SetOptions { job_id, options }).await
    }

    pub async fn analyse(&self, job_id: String) -> Result<AnalysisSummary, ErrorBody> {
        self.call(Request::Analyse { job_id }).await
    }

    pub async fn rule_counts(&self, job_id: String) -> Result<Vec<RuleCount>, ErrorBody> {
        self.call(Request::RuleCounts { job_id }).await
    }

    pub async fn issues(
        &self,
        job_id: String,
        rule: Option<String>,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<IssueView>, ErrorBody> {
        self.call(Request::Issues {
            job_id,
            rule,
            after_id,
            limit,
        })
        .await
    }

    pub async fn decide(
        &self,
        job_id: String,
        issue_id: i64,
        decision: DecisionInput,
    ) -> Result<(), ErrorBody> {
        self.call(Request::Decide {
            job_id,
            issue_id,
            decision,
        })
        .await
    }

    /// Returns how many issues changed.
    pub async fn decide_all(
        &self,
        job_id: String,
        rule: Option<String>,
        decision: DecisionInput,
    ) -> Result<u64, ErrorBody> {
        self.call(Request::DecideAll {
            job_id,
            rule,
            decision,
        })
        .await
    }

    pub async fn start(&self, job_id: String) -> Result<(), ErrorBody> {
        self.call(Request::Start { job_id }).await
    }

    pub async fn pause(&self, job_id: String) -> Result<(), ErrorBody> {
        self.call(Request::Pause { job_id }).await
    }

    pub async fn resume(&self, job_id: String) -> Result<(), ErrorBody> {
        self.call(Request::Resume { job_id }).await
    }

    pub async fn cancel(&self, job_id: String) -> Result<(), ErrorBody> {
        self.call(Request::Cancel { job_id }).await
    }

    pub async fn list_jobs(&self, limit: u32, offset: u32) -> Result<Vec<JobSummary>, ErrorBody> {
        self.call(Request::ListJobs { limit, offset }).await
    }

    pub async fn job(&self, job_id: String) -> Result<JobSummary, ErrorBody> {
        self.call(Request::Job { job_id }).await
    }
}
