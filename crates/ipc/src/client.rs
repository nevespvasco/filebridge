//! The client side: one connection, concurrent calls matched by id, and an event stream.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use interprocess::local_socket::tokio::{Stream, prelude::*};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};

use crate::{Endpoint, Envelope, ErrorBody, Event, Outgoing, Request};

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

/// A connection to `filebridge-service`. Cheap to clone; clones share the connection. Every
/// operation is a method of [`Operations`](crate::Operations), which it implements.
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
        let (recv, send) = conn.split();
        Ok(Self::over(recv, send))
    }

    /// A client over a connection opened elsewhere, e.g. a Unix socket whose peer process was
    /// looked up first (the app replacing an older service checks the process it pinged).
    pub fn over(
        recv: impl AsyncRead + Unpin + Send + 'static,
        mut send: impl AsyncWrite + Unpin + Send + 'static,
    ) -> Self {
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

        Self {
            out,
            pending,
            events,
            next_id: Arc::new(AtomicU64::new(1)),
            subscribed: Arc::default(),
        }
    }

    /// Sends a request and decodes its answer.
    pub(crate) async fn call<T: DeserializeOwned>(&self, request: Request) -> Result<T, ErrorBody> {
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
}
