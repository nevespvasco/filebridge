//! The connection to `filebridge-service` (ADR 0008): connect to the per-user socket, start the
//! service when nothing listens there, forward its events to the window and reconnect when the
//! connection drops. A service from an older version of the app (an older protocol) is stopped
//! and replaced; its running jobs resume in the new one. A newer one is never touched.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use filebridge_ipc::{Client, Endpoint, ErrorBody, Operations, PROTOCOL_VERSION, Pong};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_specta::Event as _;
use tokio::sync::{Mutex, broadcast};
use tokio::time::{Instant, sleep};

/// The service binary's file name.
const SERVICE_BIN: &str = if cfg!(windows) {
    "filebridge-service.exe"
} else {
    "filebridge-service"
};
/// How long to wait for a freshly started service to accept connections.
const START_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an older service gets to stop (it stores running jobs as they are, then exits).
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Every engine event, forwarded as is from `Client::events()`.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct ServiceEvent(pub filebridge_ipc::Event);

/// The window's link to the service went up or down. After `connected: true` the UI reloads
/// what it shows, because events sent while disconnected are lost.
#[derive(Debug, Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct ConnectionChanged {
    pub connected: bool,
    /// Socket path or pipe name, for the Details panel.
    pub endpoint: String,
}

/// Shared handle, kept in Tauri's state.
#[derive(Clone)]
pub struct Service(Arc<Inner>);

struct Inner {
    app: AppHandle,
    endpoint: Endpoint,
    /// The live client and its generation. The lock is held while connecting, so concurrent
    /// commands never start the service twice.
    current: Mutex<Option<(u64, Client)>>,
    generation: AtomicU64,
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
            endpoint: endpoint(),
            current: Mutex::new(None),
            generation: Default::default(),
        }))
    }

    pub fn endpoint(&self) -> String {
        self.0.endpoint.display()
    }

    /// The connected client, connecting (and starting the service) first if needed.
    ///
    /// Boxed because it and the event forwarder it starts call each other, which would make
    /// the `Send` bound of the future impossible to infer.
    pub fn client(&self) -> Pin<Box<dyn Future<Output = Result<Client, ErrorBody>> + Send + '_>> {
        Box::pin(async move {
            let mut current = self.0.current.lock().await;
            if let Some((_, client)) = current.as_ref() {
                return Ok(client.clone());
            }
            let client = self.connect_or_start().await?;
            let generation = self.0.generation.fetch_add(1, Ordering::Relaxed);
            *current = Some((generation, client.clone()));
            tauri::async_runtime::spawn(self.clone().forward_events(generation, client.clone()));
            Ok(client)
        })
    }

    /// Drops the client of `generation` (if it is still the current one).
    async fn forget(&self, generation: u64) {
        let mut current = self.0.current.lock().await;
        if current.as_ref().is_some_and(|(g, _)| *g == generation) {
            *current = None;
        }
    }

    async fn connect_or_start(&self) -> Result<Client, ErrorBody> {
        let endpoint = &self.0.endpoint;
        if let Ok(client) = Client::connect(endpoint).await {
            let pong = client.ping().await?;
            if pong.protocol >= PROTOCOL_VERSION {
                return check_protocol(client, &pong);
            }
            // Left running by an older version of the app: replace it.
            drop(client);
            tracing::info!(
                "replacing filebridge-service {} (protocol {})",
                pong.version,
                pong.protocol
            );
            stop_older(endpoint).await.map_err(|why| ErrorBody {
                code: "incompatible_service".into(),
                message: format!("{}; it could not be stopped: {why}", incompatible(&pong)),
            })?;
        }
        let bin = find_service_binary().ok_or_else(|| {
            unavailable(format!(
                "{SERVICE_BIN} is not running and was not found next to the app or on PATH"
            ))
        })?;
        tracing::info!("starting {}", bin.display());
        spawn_detached(&bin)
            .map_err(|e| unavailable(format!("could not start {}: {e}", bin.display())))?;
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            sleep(Duration::from_millis(100)).await;
            match Client::connect(endpoint).await {
                Ok(client) => {
                    let pong = client.ping().await?;
                    return check_protocol(client, &pong);
                }
                Err(e) if Instant::now() >= deadline => {
                    return Err(unavailable(format!(
                        "{} did not answer on {} within {} s: {e}",
                        bin.display(),
                        endpoint.display(),
                        START_TIMEOUT.as_secs()
                    )));
                }
                Err(_) => {}
            }
        }
    }

    /// Forwards events until the connection drops (the event stream then reports `Closed`),
    /// then reconnects in the background.
    async fn forward_events(self, generation: u64, client: Client) {
        let app = &self.0.app;
        match client.events().await {
            Ok(mut rx) => {
                self.emit_connection(true);
                loop {
                    match rx.recv().await {
                        Ok(event) => {
                            if let Err(e) = ServiceEvent(event).emit(app) {
                                tracing::warn!("could not forward an event: {e}");
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::debug!("the window missed {n} progress events");
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
            Err(e) => tracing::warn!("could not subscribe to events: {e}"),
        }
        self.forget(generation).await;
        self.emit_connection(false);

        let mut delay = Duration::from_secs(1);
        loop {
            sleep(delay).await;
            // `client()` starts a new forwarder once it connects.
            if self.client().await.is_ok() {
                return;
            }
            delay = (delay * 2).min(Duration::from_secs(15));
        }
    }

    fn emit_connection(&self, connected: bool) {
        let event = ConnectionChanged {
            connected,
            endpoint: self.endpoint(),
        };
        if let Err(e) = event.emit(&self.0.app) {
            tracing::warn!("could not emit the connection state: {e}");
        }
    }
}

/// `FILEBRIDGE_SOCKET` (the same variable the service and the CLI read), else the per-user
/// default. A service started from here inherits the variable, so both sides agree.
fn endpoint() -> Endpoint {
    match std::env::var_os("FILEBRIDGE_SOCKET") {
        Some(path) => Endpoint::Path(PathBuf::from(path)),
        None => Endpoint::default_for_user(),
    }
}

fn incompatible(pong: &Pong) -> String {
    format!(
        "filebridge-service {} speaks protocol {}, this app needs {PROTOCOL_VERSION}",
        pong.version, pong.protocol
    )
}

/// Only the same protocol is spoken. A service still older after a fresh start is an old
/// binary on disk (in development: `cargo build -p filebridge-service`).
fn check_protocol(client: Client, pong: &Pong) -> Result<Client, ErrorBody> {
    if pong.protocol != PROTOCOL_VERSION {
        return Err(ErrorBody {
            code: "incompatible_service".into(),
            message: incompatible(pong),
        });
    }
    Ok(client)
}

/// Stops the service listening on `endpoint`: SIGTERM to the process at the other end of the
/// socket (checked to be a `filebridge-service`), then waits until nothing answers there.
#[cfg(unix)]
async fn stop_older(endpoint: &Endpoint) -> Result<(), String> {
    let Endpoint::Path(path) = endpoint else {
        return Err("not a socket".into());
    };
    let stream = tokio::net::UnixStream::connect(path)
        .await
        .map_err(|e| e.to_string())?;
    let pid = stream
        .peer_cred()
        .map_err(|e| e.to_string())?
        .pid()
        .ok_or("the socket doesn't say which process it belongs to")?;
    drop(stream);
    let name = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .map_err(|e| e.to_string())?;
    let name = String::from_utf8_lossy(&name.stdout);
    let name = name.trim().rsplit('/').next().unwrap_or_default();
    // `comm` is cut to 15 characters on Linux.
    if !name.starts_with("filebridge-serv") {
        return Err(format!("process {pid} ({name}) is not filebridge-service"));
    }
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("kill {pid} failed"));
    }
    let deadline = Instant::now() + STOP_TIMEOUT;
    while Client::connect(endpoint).await.is_ok() {
        if Instant::now() >= deadline {
            return Err(format!("process {pid} is still running"));
        }
        sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

/// Windows: the pipe doesn't name its process here; the user quits the old service.
#[cfg(not(unix))]
async fn stop_older(_endpoint: &Endpoint) -> Result<(), String> {
    Err("quit the running filebridge-service and try again".into())
}

/// `FILEBRIDGE_SERVICE`, then next to this executable (the bundled sidecar, and
/// `target/<profile>/` in development), then `PATH`.
fn find_service_binary() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FILEBRIDGE_SERVICE") {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(SERVICE_BIN)))
        .filter(|p| p.is_file());
    beside.or_else(|| {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|dir| dir.join(SERVICE_BIN))
            .find(|p| p.is_file())
    })
}

/// Starts the service so that it outlives this window and a Ctrl+C in the terminal that
/// started `tauri dev`.
fn spawn_detached(bin: &std::path::Path) -> std::io::Result<()> {
    let mut cmd = Command::new(bin);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = cmd.spawn()?;
    // Reap it if it exits while the window is still open.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// `target/<profile>/filebridge-service`, built by the service's own tests in the same
    /// `cargo test --workspace`.
    fn service_binary() -> Option<PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let bin = exe.parent()?.parent()?.join(SERVICE_BIN);
        bin.is_file().then_some(bin)
    }

    async fn wait_for(endpoint: &Endpoint) {
        let deadline = Instant::now() + START_TIMEOUT;
        while Client::connect(endpoint).await.is_err() {
            assert!(Instant::now() < deadline, "the service never answered");
            sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn an_older_service_is_stopped() {
        let Some(bin) = service_binary() else {
            eprintln!("skipped: build filebridge-service first");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let mut child = std::process::Command::new(bin)
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .arg("--socket")
            .arg(&socket)
            .env("FILEBRIDGE_FAKE_TRANSPORT", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let endpoint = Endpoint::Path(socket);
        wait_for(&endpoint).await;

        stop_older(&endpoint).await.unwrap();
        assert!(Client::connect(&endpoint).await.is_err());
        let status = child.wait().unwrap();
        assert!(status.success(), "a clean stop, not a kill: {status:?}");
    }

    #[tokio::test]
    async fn another_program_on_the_socket_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        // This test process listens: it is not filebridge-service.
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            while let Ok((conn, _)) = listener.accept().await {
                std::mem::forget(conn);
            }
        });
        let err = stop_older(&Endpoint::Path(socket)).await.unwrap_err();
        assert!(err.contains("is not filebridge-service"), "{err}");
    }
}
