//! The service side: accept connections and dispatch requests to the [`Engine`].

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use filebridge_core::{CoreError, Engine};
use interprocess::local_socket::ListenerOptions;
use interprocess::local_socket::tokio::{Listener, Stream, prelude::*};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};

use crate::{
    DetectNextcloudRequest, DetectNextcloudResponse, Endpoint, Envelope, ErrorBody, Outgoing,
    PROTOCOL_VERSION, Pong, Request,
};

/// Requests that need a concrete plugin, implemented by the binary that hosts the engine (it
/// is the only one allowed to know plugins).
#[async_trait]
pub trait Extensions: Send + Sync {
    async fn detect_nextcloud(
        &self,
        engine: &Arc<Engine>,
        req: DetectNextcloudRequest,
    ) -> Result<DetectNextcloudResponse, ErrorBody>;
}

/// Creates the listener. On Unix the socket's directory is created (or tightened) to 0700, so
/// only this user can connect. A stale socket file from a crashed service is replaced.
pub fn bind(endpoint: &Endpoint) -> std::io::Result<Listener> {
    if let Endpoint::Path(path) = endpoint {
        prepare_dir(path)?;
    }
    ListenerOptions::new()
        .name(endpoint.name()?)
        .try_overwrite(true)
        .create_tokio()
}

#[cfg(unix)]
fn prepare_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir: PathBuf = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn prepare_dir(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// Serves until the listener fails. Each connection runs in its own task.
pub async fn serve(
    listener: Listener,
    engine: Arc<Engine>,
    extensions: Option<Arc<dyn Extensions>>,
) -> std::io::Result<()> {
    loop {
        let conn = listener.accept().await?;
        let (engine, extensions) = (engine.clone(), extensions.clone());
        tokio::spawn(async move {
            if let Err(e) = handle_connection(conn, engine, extensions).await {
                tracing::debug!("ipc connection ended: {e}");
            }
        });
    }
}

async fn handle_connection(
    conn: Stream,
    engine: Arc<Engine>,
    extensions: Option<Arc<dyn Extensions>>,
) -> std::io::Result<()> {
    let (recv, mut send) = conn.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Outgoing>(256);
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let mut line = serde_json::to_vec(&msg).expect("messages serialize");
            line.push(b'\n');
            if send.write_all(&line).await.is_err() {
                break;
            }
        }
    });

    let mut lines = BufReader::new(recv).lines();
    let mut subscribed = false;
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let env: Envelope = match serde_json::from_str(&line) {
            Ok(env) => env,
            Err(e) => {
                let id = serde_json::from_str::<serde_json::Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").and_then(|i| i.as_u64()))
                    .unwrap_or(0);
                let _ = out_tx
                    .send(error_response(id, "invalid", format!("bad request: {e}")))
                    .await;
                continue;
            }
        };
        if matches!(env.request, Request::Subscribe) {
            if !subscribed {
                subscribed = true;
                let mut rx = engine.subscribe();
                let tx = out_tx.clone();
                tokio::spawn(async move {
                    loop {
                        match rx.recv().await {
                            Ok(event) => {
                                if tx.send(Outgoing::Event { event }).await.is_err() {
                                    break;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::debug!("ipc subscriber lagged by {n} events");
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                });
            }
            let _ = out_tx.send(ok_response(env.id, &())).await;
            continue;
        }
        let (engine, extensions, tx) = (engine.clone(), extensions.clone(), out_tx.clone());
        tokio::spawn(async move {
            let msg = match dispatch(&engine, extensions.as_deref(), env.request).await {
                Ok(value) => Outgoing::Response {
                    id: env.id,
                    ok: Some(value),
                    err: None,
                },
                Err(err) => Outgoing::Response {
                    id: env.id,
                    ok: None,
                    err: Some(err),
                },
            };
            let _ = tx.send(msg).await;
        });
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

fn ok_response<T: Serialize>(id: u64, value: &T) -> Outgoing {
    Outgoing::Response {
        id,
        ok: Some(serde_json::to_value(value).expect("responses serialize")),
        err: None,
    }
}

fn error_response(id: u64, code: &str, message: String) -> Outgoing {
    Outgoing::Response {
        id,
        ok: None,
        err: Some(ErrorBody {
            code: code.into(),
            message,
        }),
    }
}

fn to_error(e: CoreError) -> ErrorBody {
    let code = match &e {
        CoreError::PendingDecisions(_) => "pending_decisions",
        CoreError::InvalidTransition { .. } => "invalid_transition",
        CoreError::Store(filebridge_core::StoreError::NotFound(_)) => "not_found",
        CoreError::Invalid(_) => "invalid",
        _ => "internal",
    };
    ErrorBody {
        code: code.into(),
        message: e.to_string(),
    }
}

fn json<T: Serialize>(v: T) -> Result<serde_json::Value, ErrorBody> {
    serde_json::to_value(v).map_err(|e| ErrorBody {
        code: "internal".into(),
        message: e.to_string(),
    })
}

async fn dispatch(
    engine: &Arc<Engine>,
    extensions: Option<&dyn Extensions>,
    request: Request,
) -> Result<serde_json::Value, ErrorBody> {
    let e = engine;
    match request {
        Request::Ping => json(Pong {
            protocol: PROTOCOL_VERSION,
            version: env!("CARGO_PKG_VERSION").into(),
        }),
        Request::ListProfiles => json(e.list_profiles().await.map_err(to_error)?),
        Request::SaveProfile { profile } => json(e.save_profile(profile).await.map_err(to_error)?),
        Request::DeleteProfile { id } => json(e.delete_profile(&id).await.map_err(to_error)?),
        Request::TestProfile { id } => json(e.test_profile(&id).await.map_err(to_error)?),
        Request::DetectNextcloud(req) => match extensions {
            Some(x) => json(x.detect_nextcloud(engine, req).await?),
            None => Err(ErrorBody {
                code: "unsupported".into(),
                message: "this service has no Nextcloud support".into(),
            }),
        },
        Request::CreateJob {
            sources,
            profile_id,
            options,
        } => json(
            e.create_job(
                sources.into_iter().map(PathBuf::from).collect(),
                &profile_id,
                options,
            )
            .await
            .map_err(to_error)?,
        ),
        Request::SetOptions { job_id, options } => {
            json(e.set_options(&job_id, options).await.map_err(to_error)?)
        }
        Request::Analyse { job_id } => json(e.analyse(&job_id).await.map_err(to_error)?),
        Request::RuleCounts { job_id } => json(e.rule_counts(&job_id).await.map_err(to_error)?),
        Request::Issues {
            job_id,
            rule,
            after_id,
            limit,
        } => json(
            e.issues(&job_id, rule, after_id, limit.min(1000))
                .await
                .map_err(to_error)?,
        ),
        Request::Decide {
            job_id,
            issue_id,
            decision,
        } => json(
            e.decide_issue(&job_id, issue_id, decision)
                .await
                .map_err(to_error)?,
        ),
        Request::DecideAll {
            job_id,
            rule,
            decision,
        } => json(
            e.decide_all(&job_id, rule, decision)
                .await
                .map_err(to_error)?,
        ),
        Request::Start { job_id } => json(e.start(&job_id).await.map_err(to_error)?),
        Request::Pause { job_id } => json(e.pause(&job_id).await.map_err(to_error)?),
        Request::Resume { job_id } => json(e.resume(&job_id).await.map_err(to_error)?),
        Request::Cancel { job_id } => json(e.cancel(&job_id).await.map_err(to_error)?),
        Request::ListJobs { limit, offset } => json(
            e.list_jobs(limit.min(1000), offset)
                .await
                .map_err(to_error)?,
        ),
        Request::Job { job_id } => json(e.job(&job_id).await.map_err(to_error)?),
        Request::Subscribe => unreachable!("handled per connection"),
    }
}
