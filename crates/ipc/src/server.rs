//! The service side: accept connections and run each request against a [`Handler`].

use std::path::PathBuf;
use std::sync::Arc;

use interprocess::local_socket::ListenerOptions;
use interprocess::local_socket::tokio::{Listener, Stream, prelude::*};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};

use crate::operations::dispatch;
use crate::{Endpoint, Envelope, ErrorBody, Event, Operations, Outgoing, Request};

/// What the service serves: every operation, and the engine's events for subscribers.
/// Implemented by the binary that hosts the engine (`crates/host`), the only one allowed to know
/// plugins.
pub trait Handler: Operations {
    fn subscribe(&self) -> broadcast::Receiver<Event>;
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
pub async fn serve(listener: Listener, handler: Arc<dyn Handler>) -> std::io::Result<()> {
    loop {
        let conn = listener.accept().await?;
        let handler = handler.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(conn, handler).await {
                tracing::debug!("ipc connection ended: {e}");
            }
        });
    }
}

async fn handle_connection(conn: Stream, handler: Arc<dyn Handler>) -> std::io::Result<()> {
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
                let mut rx = handler.subscribe();
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
        let (handler, tx) = (handler.clone(), out_tx.clone());
        tokio::spawn(async move {
            let msg = match dispatch(handler.as_ref(), env.request).await {
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
