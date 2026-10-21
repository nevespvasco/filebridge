//! In-memory [`RemoteShell`] for tests: answers are keyed by the exact command string, and every
//! command run is recorded. Public so `core` and the CLI can test against it too.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use filebridge_integration_api::{IntegrationError, Result};
use tokio::sync::mpsc;

use crate::shell::{Output, RemoteShell, ShellConnector, SshTarget};

/// Exit status of a command the fake has no answer for (same as "command not found").
pub const UNEXPECTED_STATUS: i32 = 127;

#[derive(Debug, Clone)]
struct Answer {
    output: Output,
    /// Pause between stdout lines in `run_lines` (use with paused tokio time).
    line_delay: Duration,
}

#[derive(Debug, Default)]
pub struct FakeShell {
    answers: Mutex<HashMap<String, Answer>>,
    ran: Mutex<Vec<String>>,
    unreachable: bool,
}

impl FakeShell {
    pub fn new() -> Self {
        Self::default()
    }

    /// A shell whose every command fails to run (connection lost).
    pub fn unreachable() -> Self {
        Self {
            unreachable: true,
            ..Self::default()
        }
    }

    /// Answers `cmd` with `output` (replacing any earlier answer).
    pub fn on(&self, cmd: impl Into<String>, output: Output) -> &Self {
        self.on_slow(cmd, output, Duration::ZERO)
    }

    /// Like [`FakeShell::on`], pausing `line_delay` before each stdout line in `run_lines`.
    pub fn on_slow(&self, cmd: impl Into<String>, output: Output, line_delay: Duration) -> &Self {
        self.answers
            .lock()
            .unwrap()
            .insert(cmd.into(), Answer { output, line_delay });
        self
    }

    /// Every command run so far, in order.
    pub fn ran(&self) -> Vec<String> {
        self.ran.lock().unwrap().clone()
    }

    /// Commands run that had no answer.
    pub fn unexpected(&self) -> Vec<String> {
        let answers = self.answers.lock().unwrap();
        self.ran()
            .into_iter()
            .filter(|c| !answers.contains_key(c))
            .collect()
    }

    fn answer(&self, cmd: &str) -> Result<Answer> {
        if self.unreachable {
            return Err(IntegrationError::retryable("connection refused (fake)"));
        }
        self.ran.lock().unwrap().push(cmd.to_string());
        Ok(self
            .answers
            .lock()
            .unwrap()
            .get(cmd)
            .cloned()
            .unwrap_or_else(|| Answer {
                output: Output::fail(UNEXPECTED_STATUS, format!("FakeShell: no answer for {cmd}")),
                line_delay: Duration::ZERO,
            }))
    }
}

#[async_trait]
impl RemoteShell for FakeShell {
    async fn run(&self, cmd: &str) -> Result<Output> {
        Ok(self.answer(cmd)?.output)
    }

    async fn run_lines(&self, cmd: &str, lines: mpsc::UnboundedSender<String>) -> Result<Output> {
        let answer = self.answer(cmd)?;
        for line in answer.output.stdout.lines() {
            if !answer.line_delay.is_zero() {
                tokio::time::sleep(answer.line_delay).await;
            }
            let _ = lines.send(line.to_string());
        }
        Ok(answer.output)
    }
}

/// Hands out the same [`FakeShell`] for every target, and records the targets.
#[derive(Debug, Clone)]
pub struct FakeConnector {
    pub shell: Arc<FakeShell>,
    targets: Arc<Mutex<Vec<SshTarget>>>,
    refuse: bool,
}

impl FakeConnector {
    pub fn new(shell: Arc<FakeShell>) -> Self {
        Self {
            shell,
            targets: Arc::default(),
            refuse: false,
        }
    }

    /// A connector whose every connection fails.
    pub fn refusing() -> Self {
        Self {
            refuse: true,
            ..Self::new(Arc::new(FakeShell::new()))
        }
    }

    pub fn targets(&self) -> Vec<SshTarget> {
        self.targets.lock().unwrap().clone()
    }
}

#[async_trait]
impl ShellConnector for FakeConnector {
    async fn connect(&self, target: &SshTarget) -> Result<Arc<dyn RemoteShell>> {
        self.targets.lock().unwrap().push(target.clone());
        if self.refuse {
            return Err(IntegrationError::retryable(format!(
                "could not connect to {}:{} (fake)",
                target.host, target.port
            )));
        }
        Ok(self.shell.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_by_command_and_records() {
        let shell = FakeShell::new();
        shell.on("pct list", Output::ok("VMID\n"));
        assert_eq!(shell.run("pct list").await.unwrap().stdout, "VMID\n");
        let miss = shell.run("rm -rf /").await.unwrap();
        assert_eq!(miss.status, UNEXPECTED_STATUS);
        assert_eq!(shell.ran(), ["pct list", "rm -rf /"]);
        assert_eq!(shell.unexpected(), ["rm -rf /"]);
    }

    #[tokio::test]
    async fn run_lines_streams_stdout() {
        let shell = FakeShell::new();
        shell.on("x", Output::ok("a\nb\n"));
        let (tx, mut rx) = mpsc::unbounded_channel();
        shell.run_lines("x", tx).await.unwrap();
        assert_eq!(rx.recv().await.as_deref(), Some("a"));
        assert_eq!(rx.recv().await.as_deref(), Some("b"));
        assert_eq!(rx.recv().await, None);
    }
}
