//! [`RemoteShell`] over SSH (russh). Host keys are verified against `known_hosts`; there is no
//! "accept unknown host" path — the user adds the host with `ssh` first, or the profile wizard
//! will (Phase 2). Authentication: an unencrypted key file, or the SSH agent.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use filebridge_integration_api::{IntegrationError, Result};
use russh::ChannelMsg;
use russh::ChannelWriteHalf;
use russh::client;
use russh::keys::agent::AgentIdentity;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use tokio::sync::mpsc;

use crate::shell::{Output, RemoteShell, ShellConnector, SshTarget};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Opens [`SshShell`]s.
#[derive(Debug, Clone, Default)]
pub struct SshConnector;

impl SshConnector {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ShellConnector for SshConnector {
    async fn connect(&self, target: &SshTarget) -> Result<Arc<dyn RemoteShell>> {
        Ok(Arc::new(SshShell::connect(target).await?))
    }
}

/// Why the host key was rejected, kept by the handler so the error can say it.
#[derive(Debug, Clone)]
enum HostKeyProblem {
    Unknown { fingerprint: String },
    Changed { line: usize },
    Certificate,
    Unreadable(String),
}

struct KnownHostsCheck {
    host: String,
    port: u16,
    known_hosts: PathBuf,
    problem: Arc<Mutex<Option<HostKeyProblem>>>,
}

impl client::Handler for KnownHostsCheck {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = server_key else {
            *self.problem.lock().unwrap() = Some(HostKeyProblem::Certificate);
            return Ok(false);
        };
        let problem = match russh::keys::check_known_hosts_path(
            &self.host,
            self.port,
            key,
            &self.known_hosts,
        ) {
            Ok(true) => return Ok(true),
            Ok(false) => HostKeyProblem::Unknown {
                fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            },
            Err(russh::keys::Error::KeyChanged { line }) => HostKeyProblem::Changed { line },
            Err(e) => HostKeyProblem::Unreadable(e.to_string()),
        };
        *self.problem.lock().unwrap() = Some(problem);
        Ok(false)
    }
}

/// Dropping it closes the connection.
pub struct SshShell {
    handle: client::Handle<KnownHostsCheck>,
}

impl SshShell {
    pub async fn connect(target: &SshTarget) -> Result<Self> {
        let known_hosts = match &target.known_hosts_file {
            Some(p) => p.clone(),
            None => std::env::home_dir()
                .map(|h| h.join(".ssh").join("known_hosts"))
                .ok_or_else(|| IntegrationError::fatal("no home directory for known_hosts"))?,
        };
        let problem = Arc::new(Mutex::new(None));
        let handler = KnownHostsCheck {
            host: target.host.clone(),
            port: target.port,
            known_hosts: known_hosts.clone(),
            problem: problem.clone(),
        };
        let config = Arc::new(client::Config {
            // Scans can be silent for minutes on a slow disk: keep the link alive, never time out.
            inactivity_timeout: None,
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 4,
            ..Default::default()
        });
        let addr = (target.host.as_str(), target.port);
        let connected =
            tokio::time::timeout(CONNECT_TIMEOUT, client::connect(config, addr, handler)).await;
        let mut handle = match connected {
            Err(_) => {
                return Err(IntegrationError::retryable(format!(
                    "SSH connection to {}:{} timed out",
                    target.host, target.port
                )));
            }
            Ok(Err(e)) => {
                let found = problem.lock().unwrap().clone();
                return Err(match found {
                    Some(p) => host_key_error(target, &known_hosts, p),
                    None => IntegrationError::retryable(format!(
                        "SSH connection to {}:{} failed: {e}",
                        target.host, target.port
                    )),
                });
            }
            Ok(Ok(h)) => h,
        };
        authenticate(&mut handle, target).await?;
        Ok(Self { handle })
    }
}

fn host_key_error(
    target: &SshTarget,
    known_hosts: &std::path::Path,
    problem: HostKeyProblem,
) -> IntegrationError {
    let host = &target.host;
    let file = known_hosts.display();
    IntegrationError::fatal(match problem {
        HostKeyProblem::Unknown { fingerprint } => format!(
            "host key of {host} is not in {file} ({fingerprint}); connect once with `ssh` to \
             check and add it"
        ),
        HostKeyProblem::Changed { line } => format!(
            "host key of {host} does not match {file} line {line}: the server changed or the \
             connection is being intercepted"
        ),
        HostKeyProblem::Certificate => {
            format!("{host} offered a host certificate; only plain host keys are supported")
        }
        HostKeyProblem::Unreadable(e) => format!("could not check {file}: {e}"),
    })
}

async fn authenticate(
    handle: &mut client::Handle<KnownHostsCheck>,
    target: &SshTarget,
) -> Result<()> {
    let user = target.user.as_str();
    let net = |e: russh::Error| IntegrationError::retryable(format!("SSH authentication: {e}"));
    let rsa_hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(net)?
        .flatten();

    if let Some(path) = &target.key_file {
        let key = russh::keys::load_secret_key(path, None).map_err(|e| {
            IntegrationError::fatal(format!(
                "could not load SSH key {} ({e}); encrypted keys must be loaded into the SSH agent",
                path.display()
            ))
        })?;
        let res = handle
            .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash))
            .await
            .map_err(net)?;
        return if res.success() {
            Ok(())
        } else {
            Err(IntegrationError::fatal(format!(
                "server refused key {} for user {user}",
                path.display()
            )))
        };
    }

    let mut agent = connect_agent().await?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|e| IntegrationError::fatal(format!("SSH agent: {e}")))?;
    for identity in identities {
        let AgentIdentity::PublicKey { key, .. } = identity else {
            continue;
        };
        let res = handle
            .authenticate_publickey_with(user, key, rsa_hash, &mut agent)
            .await
            .map_err(|e| IntegrationError::retryable(format!("SSH agent signing: {e}")))?;
        if res.success() {
            return Ok(());
        }
    }
    Err(IntegrationError::fatal(format!(
        "server refused every SSH agent key for user {user}"
    )))
}

#[cfg(unix)]
async fn connect_agent() -> Result<
    russh::keys::agent::client::AgentClient<
        Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin>,
    >,
> {
    let agent = russh::keys::agent::client::AgentClient::connect_env()
        .await
        .map_err(|e| {
            IntegrationError::fatal(format!(
                "no SSH key file in the profile and no SSH agent ({e})"
            ))
        })?;
    Ok(agent.dynamic())
}

#[cfg(windows)]
async fn connect_agent() -> Result<
    russh::keys::agent::client::AgentClient<
        Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin>,
    >,
> {
    let agent =
        russh::keys::agent::client::AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent")
            .await
            .map_err(|e| {
                IntegrationError::fatal(format!(
                    "no SSH key file in the profile and no OpenSSH agent ({e})"
                ))
            })?;
    Ok(agent.dynamic())
}

/// Splits a byte stream into lines, decoding each lossily (paths are UTF-8 on Nextcloud).
#[derive(Default)]
struct LineSplitter {
    pending: Vec<u8>,
}

impl LineSplitter {
    fn push(&mut self, data: &[u8], lines: &mpsc::UnboundedSender<String>) {
        self.pending.extend_from_slice(data);
        while let Some(pos) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]);
            let _ = lines.send(line.trim_end_matches('\r').to_string());
        }
    }

    fn finish(self, lines: &mpsc::UnboundedSender<String>) {
        if !self.pending.is_empty() {
            let _ = lines.send(String::from_utf8_lossy(&self.pending).into_owned());
        }
    }
}

/// Closes the channel when a command is dropped before it ends (the job was cancelled), so sshd
/// closes the command's stdin and a [`crate::profile::stop_on_eof`] command stops, even if the
/// connection itself stays open.
struct CloseOnDrop(Option<ChannelWriteHalf<client::Msg>>);

impl CloseOnDrop {
    fn channel(&self) -> &ChannelWriteHalf<client::Msg> {
        self.0.as_ref().expect("taken only by disarm and drop")
    }

    /// The command ended and the server closed the channel: nothing left to close.
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        if let Some(write) = self.0.take()
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            rt.spawn(async move {
                let _ = write.close().await;
            });
        }
    }
}

#[async_trait]
impl RemoteShell for SshShell {
    async fn run(&self, cmd: &str) -> Result<Output> {
        let (tx, _rx) = mpsc::unbounded_channel();
        self.run_lines(cmd, tx).await
    }

    async fn run_lines(&self, cmd: &str, lines: mpsc::UnboundedSender<String>) -> Result<Output> {
        let lost = |e: russh::Error| IntegrationError::retryable(format!("SSH: {e}"));
        tracing::debug!(command = cmd, "ssh exec");
        let (mut channel, write) = self
            .handle
            .channel_open_session()
            .await
            .map_err(lost)?
            .split();
        let close = CloseOnDrop(Some(write));
        close.channel().exec(true, cmd).await.map_err(lost)?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut status = None;
        let mut splitter = LineSplitter::default();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    splitter.push(&data, &lines);
                    stdout.extend_from_slice(&data);
                }
                ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => {
                    status = Some(i32::try_from(exit_status).unwrap_or(-1));
                }
                _ => {}
            }
        }
        splitter.finish(&lines);
        close.disarm();
        Ok(Output {
            status: status.unwrap_or(-1),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_split_across_chunks() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut s = LineSplitter::default();
        let text = "\tFile\t/alice/files/Verão.jpg\r\nsecond\nlast";
        // Split inside the two-byte "ã" and inside "\r\n".
        let bytes = text.as_bytes();
        let cut1 = text.find('ã').unwrap() + 1;
        let cut2 = text.find('\n').unwrap();
        for part in [&bytes[..cut1], &bytes[cut1..cut2], &bytes[cut2..]] {
            s.push(part, &tx);
        }
        s.finish(&tx);
        drop(tx);
        let mut got = Vec::new();
        while let Ok(l) = rx.try_recv() {
            got.push(l);
        }
        assert_eq!(got, ["\tFile\t/alice/files/Verão.jpg", "second", "last"]);
    }
}
