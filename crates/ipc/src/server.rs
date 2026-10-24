//! The service side: accept connections and run each request against a [`Handler`].

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

/// Creates the listener, reachable only by this user: on Unix the socket's directory must be
/// this user's and is tightened to 0700; on Windows the pipe's DACL admits only this user's SID.
/// A stale socket file from a crashed service is replaced.
pub fn bind(endpoint: &Endpoint) -> std::io::Result<Listener> {
    if let Endpoint::Path(path) = endpoint {
        prepare_dir(path)?;
    }
    let options = ListenerOptions::new()
        .name(endpoint.name()?)
        .try_overwrite(true);
    #[cfg(windows)]
    let options = {
        use interprocess::os::windows::local_socket::ListenerOptionsExt;
        options.security_descriptor(crate::peer::user_only()?)
    };
    options.create_tokio()
}

#[cfg(unix)]
fn prepare_dir(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::path::PathBuf;
    let dir: PathBuf = path
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    // Not followed: a directory someone else made (or a link to one) is refused, so another
    // user can't hold the socket's place.
    let meta = std::fs::symlink_metadata(&dir)?;
    // SAFETY: geteuid has no preconditions and can't fail.
    let me = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != me {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "{} is not a directory owned by this user; remove it and start again",
                dir.display()
            ),
        ));
    }
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_socket_directory_is_tightened_to_0700() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("run");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        prepare_dir(&dir.join("service.sock")).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn a_link_in_place_of_the_socket_directory_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        let dir = tmp.path().join("run");
        std::os::unix::fs::symlink(&elsewhere, &dir).unwrap();
        let err = prepare_dir(&dir.join("service.sock")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }
}
