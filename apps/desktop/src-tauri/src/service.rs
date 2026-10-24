//! The connection to `filebridge-service` (ADR 0008): connect to the per-user socket, start the
//! service when nothing listens there, forward its events to the window and reconnect when the
//! connection drops. A service from an older version of the app (an older protocol) is stopped
//! and replaced, started again with the same data folder, rclone and socket; its running jobs
//! resume in the new one. A newer one is never touched, and neither is one started by the
//! systemd user unit or with arguments this app can't pass on: the user is told instead.

use std::ffi::OsString;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// An older service is being stopped and replaced: every command waits until it is done
    /// (up to [`STOP_TIMEOUT`] plus [`START_TIMEOUT`]).
    pub replacing: bool,
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
    /// Set once a replacement still spoke an older protocol: the binary on disk is old too, so
    /// replacing again would only stop and start it over and over.
    gave_up_replacing: AtomicBool,
    /// An older service is being replaced right now.
    replacing: AtomicBool,
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
            gave_up_replacing: AtomicBool::new(false),
            replacing: AtomicBool::new(false),
        }))
    }

    pub fn endpoint(&self) -> String {
        self.0.endpoint.display()
    }

    /// Whether an older service is being replaced now. The window asks once it listens for
    /// [`ConnectionChanged`]: a replacement can start before it does.
    pub fn replacing(&self) -> bool {
        self.0.replacing.load(Ordering::Relaxed)
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

    /// Says so while an older service is replaced; once it is done, the forwarder's
    /// `connected: true`, or here `connected: false` when it failed, ends it.
    async fn connect_or_start(&self) -> Result<Client, ErrorBody> {
        let on_replace = || {
            self.0.replacing.store(true, Ordering::Relaxed);
            self.emit_connection(false);
        };
        let result = Starter {
            endpoint: &self.0.endpoint,
            protocol: PROTOCOL_VERSION,
            find_binary: &find_service_binary,
            gave_up: &self.0.gave_up_replacing,
            on_replace: &on_replace,
        }
        .connect_or_start()
        .await;
        if self.0.replacing.swap(false, Ordering::Relaxed) && result.is_err() {
            self.emit_connection(false);
        }
        result
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
            replacing: self.replacing(),
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

fn incompatible(pong: &Pong, protocol: u32) -> String {
    format!(
        "filebridge-service {} speaks protocol {}, this app needs {protocol}",
        pong.version, pong.protocol
    )
}

fn incompatible_service(message: String) -> ErrorBody {
    ErrorBody {
        code: "incompatible_service".into(),
        message,
    }
}

/// Only the same protocol is spoken. A service still older after a fresh start is an old
/// binary on disk (in development: `cargo build -p filebridge-service`).
fn check_protocol(client: Client, pong: &Pong, protocol: u32) -> Result<Client, ErrorBody> {
    if pong.protocol != protocol {
        return Err(incompatible_service(incompatible(pong, protocol)));
    }
    Ok(client)
}

/// Connecting, starting and replacing the service, apart from the window (tests stand in for
/// the app's protocol and the binary it finds).
struct Starter<'a> {
    endpoint: &'a Endpoint,
    /// The protocol this app speaks.
    protocol: u32,
    find_binary: &'a (dyn Fn() -> Option<PathBuf> + Send + Sync),
    gave_up: &'a AtomicBool,
    /// Called once an older service is about to be stopped and replaced.
    on_replace: &'a (dyn Fn() + Send + Sync),
}

impl Starter<'_> {
    async fn connect_or_start(&self) -> Result<Client, ErrorBody> {
        let endpoint = self.endpoint;
        let client = match Client::connect(endpoint).await {
            Ok(client) => client,
            // Another user's process holds the name: starting ours wouldn't take it back.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Err(unavailable(format!("{}: {e}", endpoint.display())));
            }
            Err(_) => {
                let bin = (self.find_binary)().ok_or_else(|| {
                    unavailable(format!(
                        "{SERVICE_BIN} is not running and was not found next to the app or on PATH"
                    ))
                })?;
                let (client, pong) = self.start(&bin, &Launch::default()).await?;
                return check_protocol(client, &pong, self.protocol);
            }
        };
        let pong = client.ping().await?;
        if pong.protocol >= self.protocol {
            return check_protocol(client, &pong, self.protocol);
        }
        drop(client);
        // Left running by an older version of the app: replace it, unless that was tried.
        let old = incompatible(&pong, self.protocol);
        if self.gave_up.load(Ordering::Relaxed) {
            return Err(incompatible_service(format!(
                "{old}; the {SERVICE_BIN} this app starts is just as old: update it"
            )));
        }
        // Found before the old one is stopped: with nothing to start, it keeps running.
        let bin = (self.find_binary)().ok_or_else(|| {
            incompatible_service(format!(
                "{old}; it was left running because no {SERVICE_BIN} was found next to the app or on PATH to replace it"
            ))
        })?;
        tracing::info!(
            "replacing filebridge-service {} (protocol {})",
            pong.version,
            pong.protocol
        );
        (self.on_replace)();
        let launch = match stop_older(endpoint, self.protocol).await {
            Ok(Stopped::Replaced(launch)) => launch,
            // Another window replaced it in the meantime.
            Ok(Stopped::Current) => {
                let client = Client::connect(endpoint)
                    .await
                    .map_err(|e| unavailable(e.to_string()))?;
                let pong = client.ping().await?;
                return check_protocol(client, &pong, self.protocol);
            }
            Err(why) => {
                return Err(incompatible_service(format!(
                    "{old}; it could not be replaced: {why}"
                )));
            }
        };
        let (client, pong) = self.start(&bin, &launch).await?;
        if pong.protocol < self.protocol {
            self.gave_up.store(true, Ordering::Relaxed);
        }
        check_protocol(client, &pong, self.protocol)
    }

    /// Starts `bin` and waits until it answers.
    async fn start(
        &self,
        bin: &std::path::Path,
        launch: &Launch,
    ) -> Result<(Client, Pong), ErrorBody> {
        let endpoint = self.endpoint;
        tracing::info!("starting {}", bin.display());
        spawn_detached(bin, launch)
            .map_err(|e| unavailable(format!("could not start {}: {e}", bin.display())))?;
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            sleep(Duration::from_millis(100)).await;
            match Client::connect(endpoint).await {
                Ok(client) => {
                    let pong = client.ping().await?;
                    return Ok((client, pong));
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::PermissionDenied
                        || Instant::now() >= deadline =>
                {
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
}

/// How a service is started: its arguments, and the `FILEBRIDGE_*` variables it gets
/// (`None`: this app's own environment).
#[derive(Debug, Default, PartialEq)]
struct Launch {
    args: Vec<OsString>,
    env: Option<Vec<(OsString, OsString)>>,
}

/// The variables the service reads; a replacement gets the old service's values, and only those.
const SERVICE_VARS: [&str; 5] = [
    "FILEBRIDGE_DATA_DIR",
    "FILEBRIDGE_RCLONE",
    "FILEBRIDGE_SOCKET",
    "FILEBRIDGE_FAKE_TRANSPORT",
    "FILEBRIDGE_LOG",
];

/// The systemd user unit `filebridge-service install-user-unit` writes.
#[cfg(any(target_os = "linux", test))]
const UNIT_NAME: &str = "filebridge-service.service";

/// How the old service was started, from its `/proc/PID/cmdline`, `environ` and `cgroup`
/// (Linux), so that its replacement uses the same data folder, rclone and socket. Refused
/// when the systemd user unit runs it (stopping it there would leave the unit stopped and a
/// copy outside it), or when it has arguments this app doesn't know how to pass on. Only Linux
/// has `/proc`; the tests run it on every Unix.
#[cfg(all(unix, any(target_os = "linux", test)))]
fn launch_from(cmdline: &[u8], environ: &[u8], cgroup: &str) -> Result<Launch, String> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    if cgroup
        .lines()
        .any(|l| l.trim_end().ends_with(&format!("/{UNIT_NAME}")))
    {
        return Err(format!(
            "the systemd user unit {UNIT_NAME} runs it: update the binary it runs, then `systemctl --user restart {UNIT_NAME}`"
        ));
    }
    let words: Vec<&[u8]> = cmdline
        .split(|b| *b == 0)
        .filter(|w| !w.is_empty())
        .collect();
    let mut args = Vec::new();
    let mut rest = words.iter().skip(1).peekable();
    while let Some(&word) = rest.next() {
        let text = String::from_utf8_lossy(word);
        let with_value = ["--data-dir", "--rclone", "--socket"];
        if with_value.contains(&text.as_ref()) {
            let value = rest
                .next()
                .ok_or_else(|| format!("it was started with {text} and no value"))?;
            args.push(OsString::from_vec(word.to_vec()));
            args.push(OsString::from_vec(value.to_vec()));
        } else if with_value
            .iter()
            .any(|f| text.starts_with(&format!("{f}=")))
            || text == "--fake-transport"
        {
            args.push(OsString::from_vec(word.to_vec()));
        } else if text == "run" && rest.peek().is_none() {
            // The default command.
        } else {
            return Err(format!(
                "it was started with arguments this app can't pass on ({}): quit it, then open the app again",
                std::ffi::OsStr::from_bytes(word).to_string_lossy()
            ));
        }
    }
    let env = environ
        .split(|b| *b == 0)
        .filter_map(|kv| {
            let eq = kv.iter().position(|b| *b == b'=')?;
            let key = std::str::from_utf8(&kv[..eq]).ok()?;
            SERVICE_VARS.contains(&key).then(|| {
                (
                    OsString::from(key),
                    OsString::from_vec(kv[eq + 1..].to_vec()),
                )
            })
        })
        .collect();
    Ok(Launch {
        args,
        env: Some(env),
    })
}

/// Linux: read from `/proc`.
#[cfg(target_os = "linux")]
fn launch_of(pid: i32) -> Result<Launch, String> {
    let read = |what: &str| {
        std::fs::read(format!("/proc/{pid}/{what}"))
            .map_err(|e| format!("could not read how process {pid} was started ({what}): {e}"))
    };
    let cgroup = String::from_utf8_lossy(&read("cgroup")?).into_owned();
    launch_from(&read("cmdline")?, &read("environ")?, &cgroup)
}

/// Other Unixes: only a service started without arguments is replaced (its environment can't
/// be read; the replacement gets this app's).
#[cfg(all(unix, not(target_os = "linux")))]
fn launch_of(pid: i32) -> Result<Launch, String> {
    let out = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output()
        .map_err(|e| e.to_string())?;
    let args = String::from_utf8_lossy(&out.stdout);
    let args = args.trim();
    if args.ends_with(SERVICE_BIN) {
        Ok(Launch::default())
    } else {
        Err(format!(
            "it was started with arguments ({args}): quit it, then open the app again"
        ))
    }
}

/// What [`stop_older`] did.
#[derive(Debug)]
enum Stopped {
    /// Stopped; the replacement is started like this.
    Replaced(Launch),
    /// The service there already speaks the protocol (another window replaced it): left alone.
    Current,
}

/// How long a service gets to answer the ping that decides whether it is stopped.
const PING_TIMEOUT: Duration = Duration::from_secs(5);

/// Stops the service listening on `endpoint` if it speaks a protocol older than `protocol`:
/// SIGTERM to the process at the other end of the socket (checked to be a
/// `filebridge-service`), then waits until nothing answers there. The process is looked up and
/// pinged on one connection, so a service that another window started in the meantime is never
/// the one stopped.
#[cfg(unix)]
async fn stop_older(endpoint: &Endpoint, protocol: u32) -> Result<Stopped, String> {
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
    let (recv, send) = stream.into_split();
    let client = Client::over(recv, send);
    let pong = tokio::time::timeout(PING_TIMEOUT, client.ping())
        .await
        .map_err(|_| format!("process {pid} did not answer"))?
        .map_err(|e| e.message)?;
    drop(client);
    if pong.protocol >= protocol {
        return Ok(Stopped::Current);
    }
    let launch = launch_of(pid)?;
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
    Ok(Stopped::Replaced(launch))
}

/// Windows: the pipe doesn't name its process here; the user quits the old service.
#[cfg(not(unix))]
async fn stop_older(_endpoint: &Endpoint, _protocol: u32) -> Result<Stopped, String> {
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
fn spawn_detached(bin: &std::path::Path, launch: &Launch) -> std::io::Result<()> {
    let mut cmd = Command::new(bin);
    cmd.args(&launch.args);
    if let Some(env) = &launch.env {
        for var in SERVICE_VARS {
            cmd.env_remove(var);
        }
        cmd.envs(env.iter().map(|(k, v)| (k, v)));
    }
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

    /// A service on a socket in `dir`, with its data there too, on the fake transport.
    fn start_service(
        bin: &std::path::Path,
        dir: &std::path::Path,
    ) -> (std::process::Child, Endpoint) {
        let socket = dir.join("s.sock");
        let child = std::process::Command::new(bin)
            .arg("--data-dir")
            .arg(dir.join("data"))
            .arg("--socket")
            .arg(&socket)
            .env("FILEBRIDGE_FAKE_TRANSPORT", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        (child, Endpoint::Path(socket))
    }

    /// The pid of the process listening on `endpoint`.
    #[cfg(target_os = "linux")]
    async fn peer(endpoint: &Endpoint) -> i32 {
        let Endpoint::Path(path) = endpoint else {
            unreachable!()
        };
        let stream = tokio::net::UnixStream::connect(path).await.unwrap();
        stream.peer_cred().unwrap().pid().unwrap()
    }

    // The test's service has arguments, which only Linux can pass on to a replacement
    // (`launch_of`); elsewhere it is left running and this would wait for it forever.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn an_older_service_is_stopped_and_its_launch_kept() {
        let Some(bin) = service_binary() else {
            eprintln!("skipped: build filebridge-service first");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (mut child, endpoint) = start_service(&bin, dir.path());
        wait_for(&endpoint).await;

        // As seen by an app one protocol newer.
        let Stopped::Replaced(launch) = stop_older(&endpoint, PROTOCOL_VERSION + 1).await.unwrap()
        else {
            panic!("not stopped");
        };
        assert!(Client::connect(&endpoint).await.is_err());
        let status = child.wait().unwrap();
        assert!(status.success(), "a clean stop, not a kill: {status:?}");
        assert_eq!(
            launch.args,
            [
                OsString::from("--data-dir"),
                dir.path().join("data").into(),
                OsString::from("--socket"),
                dir.path().join("s.sock").into(),
            ]
        );
        let env = launch.env.unwrap();
        assert!(
            env.contains(&("FILEBRIDGE_FAKE_TRANSPORT".into(), "1".into())),
            "{env:?}"
        );
        assert!(
            env.iter()
                .all(|(k, _)| SERVICE_VARS.contains(&k.to_str().unwrap()))
        );
    }

    #[tokio::test]
    async fn a_service_that_speaks_the_protocol_is_never_stopped() {
        let Some(bin) = service_binary() else {
            eprintln!("skipped: build filebridge-service first");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (mut child, endpoint) = start_service(&bin, dir.path());
        wait_for(&endpoint).await;
        // A second window that saw the old service, after the first one replaced it.
        assert!(matches!(
            stop_older(&endpoint, PROTOCOL_VERSION).await.unwrap(),
            Stopped::Current
        ));
        assert!(Client::connect(&endpoint).await.is_ok());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[tokio::test]
    async fn an_old_service_is_not_stopped_with_nothing_to_replace_it() {
        let Some(bin) = service_binary() else {
            eprintln!("skipped: build filebridge-service first");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (mut child, endpoint) = start_service(&bin, dir.path());
        wait_for(&endpoint).await;
        let gave_up = AtomicBool::new(false);
        let replaced = AtomicBool::new(false);
        let starter = Starter {
            endpoint: &endpoint,
            protocol: PROTOCOL_VERSION + 1,
            find_binary: &|| None,
            gave_up: &gave_up,
            on_replace: &|| replaced.store(true, Ordering::Relaxed),
        };
        let err = starter.connect_or_start().await.err().unwrap();
        assert_eq!(err.code, "incompatible_service");
        assert!(err.message.contains("left running"), "{}", err.message);
        assert!(
            !replaced.load(Ordering::Relaxed),
            "no replacement was announced"
        );
        assert!(child.try_wait().unwrap().is_none(), "still running");
        child.kill().unwrap();
        child.wait().unwrap();
    }

    // The test's service has arguments, which only Linux can pass on to a replacement
    // (`launch_of`); elsewhere it is left running and this would wait for it forever.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_replacement_just_as_old_is_not_replaced_again() {
        let Some(bin) = service_binary() else {
            eprintln!("skipped: build filebridge-service first");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (mut child, endpoint) = start_service(&bin, dir.path());
        wait_for(&endpoint).await;
        let gave_up = AtomicBool::new(false);
        let found = bin.clone();
        let find = move || Some(found.clone());
        // The binary on disk speaks the same (here: "old") protocol as the running one.
        let replacements = AtomicU64::new(0);
        let starter = Starter {
            endpoint: &endpoint,
            protocol: PROTOCOL_VERSION + 1,
            find_binary: &find,
            gave_up: &gave_up,
            on_replace: &|| {
                replacements.fetch_add(1, Ordering::Relaxed);
            },
        };
        let err = starter.connect_or_start().await.err().unwrap();
        assert_eq!(err.code, "incompatible_service", "{}", err.message);
        assert!(child.wait().unwrap().success(), "the first one was stopped");
        assert!(gave_up.load(Ordering::Relaxed));
        assert_eq!(
            replacements.load(Ordering::Relaxed),
            1,
            "announced before stopping"
        );
        // Its replacement was started like it, and is left alone from now on.
        assert!(dir.path().join("data").is_dir());
        let replacement = peer(&endpoint).await;
        let err = starter.connect_or_start().await.err().unwrap();
        assert!(err.message.contains("just as old"), "{}", err.message);
        assert_eq!(peer(&endpoint).await, replacement, "not stopped again");
        assert_eq!(
            replacements.load(Ordering::Relaxed),
            1,
            "nor announced again"
        );

        assert!(matches!(
            stop_older(&endpoint, u32::MAX).await.unwrap(),
            Stopped::Replaced(_)
        ));
    }

    #[test]
    fn the_old_services_arguments_are_passed_on() {
        let launch = launch_from(
            b"/opt/filebridge/filebridge-service\0--data-dir\0/srv/fb data\0--rclone=/opt/rclone\0--fake-transport\0run\0",
            b"HOME=/home/alice\0FILEBRIDGE_SOCKET=/run/user/1000/fb.sock\0PATH=/usr/bin\0",
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-filebridge.scope\n",
        )
        .unwrap();
        assert_eq!(
            launch.args,
            [
                "--data-dir",
                "/srv/fb data",
                "--rclone=/opt/rclone",
                "--fake-transport"
            ]
            .map(OsString::from)
        );
        assert_eq!(
            launch.env.unwrap(),
            [("FILEBRIDGE_SOCKET".into(), "/run/user/1000/fb.sock".into())]
        );
        // None at all: started like this app starts it.
        let launch = launch_from(b"filebridge-service\0", b"", "0::/\n").unwrap();
        assert!(launch.args.is_empty());
        assert_eq!(launch.env, Some(vec![]));
    }

    #[test]
    fn a_service_run_by_systemd_or_with_unknown_arguments_is_not_replaced() {
        let err = launch_from(
            b"/usr/bin/filebridge-service\0run\0",
            b"",
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/filebridge-service.service\n",
        )
        .unwrap_err();
        assert!(err.contains("systemctl --user restart"), "{err}");
        for cmdline in [
            &b"filebridge-service\0status\0"[..],
            b"filebridge-service\0--verbose\0",
            b"filebridge-service\0--data-dir\0",
            b"filebridge-service\0run\0--socket\0/tmp/s\0",
        ] {
            assert!(launch_from(cmdline, b"", "0::/\n").is_err(), "{cmdline:?}");
        }
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
        let err = stop_older(&Endpoint::Path(socket), u32::MAX)
            .await
            .unwrap_err();
        assert!(err.contains("is not filebridge-service"), "{err}");
    }
}
