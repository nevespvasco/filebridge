use std::net::TcpListener;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use filebridge_transport_api::{Result, TransportError};
use serde_json::json;
use tokio::process::{Child, Command};

use crate::RcClient;

/// A running `rclone rcd` sidecar. Killed when dropped.
pub struct Rcd {
    _child: Child,
    client: RcClient,
}

impl Rcd {
    /// Starts `rclone rcd` on `127.0.0.1` with a free port and random rc credentials.
    ///
    /// Credentials go through the environment, not argv, so other users can't read them from the
    /// process list. `--config ""` keeps rclone's config in memory.
    pub async fn start(binary: &Path) -> Result<Self> {
        let port = free_port()?;
        let user = random_hex(16)?;
        let pass = random_hex(32)?;
        let mut cmd = Command::new(binary);
        // If this process dies without dropping `Rcd` (SIGKILL, crash), the sidecar must not
        // outlive it. Linux: the kernel sends SIGTERM to rclone when the thread that spawned
        // it exits; that is a runtime worker thread, which lives as long as the runtime.
        // macOS/Windows: not covered yet (`kill_on_drop` only handles orderly exits).
        #[cfg(target_os = "linux")]
        unsafe {
            cmd.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd
            // filebridge owns name normalisation (`names`); rclone must compare bytes as-is.
            .args([
                "rcd",
                "--config",
                "",
                "--no-unicode-normalization",
                "--rc-addr",
            ])
            .arg(format!("127.0.0.1:{port}"))
            .env("RCLONE_RC_USER", &user)
            .env("RCLONE_RC_PASS", &pass)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // rclone logs to stderr; it goes to `tracing` (target `rclone`, debug level) instead
            // of the user's terminal. Errors that matter reach callers through the rc API.
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                TransportError::fatal(format!("cannot start {}: {e}", binary.display()))
            })?;

        let client = RcClient::new(port, user, pass);
        if let Some(stderr) = child.stderr.take() {
            // rclone's log lines may repeat a connection string or a secret: scrubbed first.
            let scrubber = client.scrubber().clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = tokio::io::BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "rclone", "{}", scrubber.scrub(&line));
                }
            });
        }

        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(TransportError::fatal(format!(
                    "rclone rcd exited: {status}"
                )));
            }
            match client.call("rc/noop", json!({})).await {
                Ok(_) => break,
                Err(e) if Instant::now() > deadline => return Err(e),
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
        Ok(Self {
            _child: child,
            client,
        })
    }

    pub fn client(&self) -> &RcClient {
        &self.client
    }
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| TransportError::fatal(format!("no free localhost port: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| TransportError::fatal(e.to_string()))?
        .port();
    Ok(port)
}

fn random_hex(len: usize) -> Result<String> {
    let mut buf = vec![0u8; len];
    getrandom::fill(&mut buf).map_err(|e| TransportError::fatal(format!("getrandom: {e}")))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}
