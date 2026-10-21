//! Running commands on the server. [`RemoteShell`] is the seam: [`crate::ssh::SshShell`] in the
//! app, [`crate::fake::FakeShell`] in tests.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use filebridge_integration_api::{IntegrationError, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Result of one remote command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// Exit status; `-1` when the command ended without one (killed by a signal).
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn fail(status: i32, stderr: impl Into<String>) -> Self {
        Self {
            status,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }

    pub fn success(&self) -> bool {
        self.status == 0
    }

    /// Last non-empty stderr line (or stdout line), for short error messages.
    pub fn short_error(&self) -> String {
        let pick = |s: &str| {
            s.lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(str::trim)
                .map(String::from)
        };
        pick(&self.stderr)
            .or_else(|| pick(&self.stdout))
            .unwrap_or_else(|| format!("exit status {}", self.status))
    }
}

#[async_trait]
pub trait RemoteShell: Send + Sync {
    /// Runs `cmd` (a full `sh` command line; quote arguments with [`crate::quote`]).
    /// `Err` only when the command could not be run at all (connection lost).
    async fn run(&self, cmd: &str) -> Result<Output>;

    /// Like [`RemoteShell::run`], and also sends each stdout line as it arrives, so long commands
    /// can report progress. The default sends the lines once the command is done.
    async fn run_lines(&self, cmd: &str, lines: mpsc::UnboundedSender<String>) -> Result<Output> {
        let out = self.run(cmd).await?;
        for line in out.stdout.lines() {
            let _ = lines.send(line.to_string());
        }
        Ok(out)
    }
}

/// Where to open a shell. Host keys are always checked against `known_hosts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshTarget {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    /// Private key file (unencrypted); when absent the SSH agent is used.
    #[serde(default)]
    pub key_file: Option<PathBuf>,
    /// Defaults to `~/.ssh/known_hosts`.
    #[serde(default)]
    pub known_hosts_file: Option<PathBuf>,
}

fn default_port() -> u16 {
    22
}

/// Opens shells. The integration is registered once; each job connects with its profile's target.
#[async_trait]
pub trait ShellConnector: Send + Sync {
    async fn connect(&self, target: &SshTarget) -> Result<Arc<dyn RemoteShell>>;
}

/// Runs `cmd` and turns a non-zero exit into a fatal error mentioning `what`.
pub(crate) async fn run_ok(shell: &dyn RemoteShell, cmd: &str, what: &str) -> Result<Output> {
    let out = shell.run(cmd).await?;
    if out.success() {
        Ok(out)
    } else {
        Err(IntegrationError::fatal(format!(
            "{what} failed: {}",
            out.short_error()
        )))
    }
}
