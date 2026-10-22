//! [`RemoteShell`] over SSH (russh). Host keys are verified against `known_hosts`, offering the
//! server only the key types on record. A connection never accepts an unknown host: the profile
//! wizards [`scan_host_key`] first and, once the user has confirmed the fingerprint,
//! [`trust_host_key`] adds it (ADR 0015); a changed key is never accepted. Authentication:
//! filebridge's own key, an unencrypted key file, or the SSH agent.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use filebridge_integration_api::{IntegrationError, Result};
use filebridge_transport_api::{KeyringRef, SecretResolver};
use russh::ChannelMsg;
use russh::ChannelWriteHalf;
use russh::client;
use russh::keys::agent::AgentIdentity;
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use tokio::sync::mpsc;
use zeroize::Zeroizing;

use crate::known_hosts::{self, Standing};
use crate::shell::{Output, RemoteShell, ShellConnector, SshTarget};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Opens [`SshShell`]s; reads a profile's own key ([`SshTarget::key`]) through the host's
/// resolver (the plugin never touches the keyring).
#[derive(Clone, Default)]
pub struct SshConnector {
    secrets: Option<Arc<dyn SecretResolver>>,
}

impl SshConnector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Profiles' own keys come through `resolver`.
    pub fn with_secrets(resolver: Arc<dyn SecretResolver>) -> Self {
        Self {
            secrets: Some(resolver),
        }
    }

    /// The private key a profile keeps in the keyring.
    async fn stored_key(&self, r: &KeyringRef) -> Result<Arc<PrivateKey>> {
        let resolver = self.secrets.as_ref().ok_or_else(|| {
            IntegrationError::fatal("no keyring available to read filebridge's SSH key")
        })?;
        // A locked keyring stays retryable; a missing key doesn't.
        let pem = resolver.resolve(r).await.map_err(|e| IntegrationError {
            message: format!("filebridge's SSH key: {}", e.message),
            retryable: e.is_retryable(),
        })?;
        let key = russh::keys::decode_secret_key(pem.expose(), None).map_err(|e| {
            IntegrationError::fatal(format!(
                "filebridge's SSH key in the keyring is unreadable: {e}"
            ))
        })?;
        Ok(Arc::new(key))
    }
}

#[async_trait]
impl ShellConnector for SshConnector {
    async fn connect(&self, target: &SshTarget) -> Result<Arc<dyn RemoteShell>> {
        let auth = match &target.key {
            Some(r) => Auth::Key(self.stored_key(r).await?),
            None => Auth::Profile,
        };
        Ok(Arc::new(SshShell::connect_auth(target, auth).await?))
    }

    async fn connect_with_key(
        &self,
        target: &SshTarget,
        key: Arc<PrivateKey>,
    ) -> Result<Arc<dyn RemoteShell>> {
        Ok(Arc::new(
            SshShell::connect_auth(target, Auth::Key(key)).await?,
        ))
    }

    async fn key_accepted(&self, target: &SshTarget, key: Arc<PrivateKey>) -> Result<bool> {
        let mut handle = SshShell::open(target).await?;
        let rsa_hash = handle
            .best_supported_rsa_hash()
            .await
            .map_err(|e| IntegrationError::retryable(format!("SSH authentication: {e}")))?
            .flatten();
        let res = handle
            .authenticate_publickey(&target.user, PrivateKeyWithHashAlg::new(key, rsa_hash))
            .await
            .map_err(|e| IntegrationError::retryable(format!("SSH authentication: {e}")))?;
        Ok(key_works(&res))
    }
}

/// Whether the server took the key: signed in, or accepted it as one factor and wants another
/// (`AuthenticationMethods publickey,password` answers "partial success"). For Remove access's
/// proof the second case still means the key works, so it is never read as "refused".
fn key_works(res: &client::AuthResult) -> bool {
    match res {
        client::AuthResult::Success => true,
        client::AuthResult::Failure {
            partial_success, ..
        } => *partial_success,
    }
}

/// How [`SshShell`] signs in.
pub enum Auth {
    /// The target's own: its key file, else the SSH agent.
    Profile,
    /// This key (filebridge's own, or one just generated).
    Key(Arc<PrivateKey>),
}

/// A new Ed25519 key for a profile: the private key in OpenSSH format (for the keyring) and the
/// public line (for `authorized_keys`), both with `comment`.
pub struct NewKey {
    pub key: Arc<PrivateKey>,
    pub pem: Zeroizing<String>,
    /// `ssh-ed25519 AAAA… <comment>`.
    pub public: String,
}

/// Generates filebridge's own key for a profile (ADR 0012), from the OS's random source.
pub fn generate_key(comment: &str) -> Result<NewKey> {
    let mut seed = Zeroizing::new([0u8; 32]);
    getrandom::fill(seed.as_mut())
        .map_err(|e| IntegrationError::fatal(format!("no random source for a new key: {e}")))?;
    let pair = russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&seed);
    let mut key = PrivateKey::from(pair);
    key.set_comment(comment);
    let pem = key
        .to_openssh(russh::keys::ssh_key::LineEnding::LF)
        .map_err(|e| IntegrationError::fatal(format!("encode the new key: {e}")))?;
    let public = key
        .public_key()
        .to_openssh()
        .map_err(|e| IntegrationError::fatal(format!("encode the new key: {e}")))?;
    Ok(NewKey {
        key: Arc::new(key),
        pem,
        public,
    })
}

/// Why the host key was rejected, kept by the handler so the error can say it.
#[derive(Debug, Clone)]
enum HostKeyProblem {
    Unknown {
        key: PublicKey,
    },
    /// `key` is `None` when the server offered none of the key types on record.
    Changed {
        line: usize,
        key: Option<PublicKey>,
    },
    Certificate,
}

/// Accepts the server's key only when `known_hosts` holds it for the host; remembers the key
/// it saw and, when rejected, why.
struct KnownHostsCheck {
    recorded: Vec<(usize, PublicKey)>,
    seen: Arc<Mutex<Option<PublicKey>>>,
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
        *self.seen.lock().unwrap() = Some(key.clone());
        let problem = match known_hosts::standing(&self.recorded, key) {
            Standing::Known => return Ok(true),
            Standing::Unknown => HostKeyProblem::Unknown { key: key.clone() },
            Standing::Changed { line } => HostKeyProblem::Changed {
                line,
                key: Some(key.clone()),
            },
        };
        *self.problem.lock().unwrap() = Some(problem);
        Ok(false)
    }
}

/// How far a connection got with the server's host key.
enum Handshake {
    /// `known_hosts` holds the key: the connection is open, not signed in yet.
    Accepted(client::Handle<KnownHostsCheck>, PublicKey),
    /// The connection was closed because of the host key.
    Rejected(HostKeyProblem),
}

/// Connects to `host:port` and checks its host key against `known_hosts`, offering only the key
/// types on record for the host (so a server with several keys presents the one `ssh` saw).
/// `Err` when the server couldn't be reached or `known_hosts` couldn't be read.
async fn handshake(host: &str, port: u16, known_hosts: &Path) -> Result<Handshake> {
    let recorded = known_hosts::recorded(host, port, known_hosts)?;
    let mut preferred = russh::Preferred::default();
    let pinned = known_hosts::host_key_algorithms(&recorded);
    if !pinned.is_empty() {
        preferred.key = pinned.into();
    }
    let first_line = recorded.first().map(|(line, _)| *line);
    let seen = Arc::new(Mutex::new(None));
    let problem = Arc::new(Mutex::new(None));
    let handler = KnownHostsCheck {
        recorded,
        seen: seen.clone(),
        problem: problem.clone(),
    };
    let config = Arc::new(client::Config {
        // Scans can be silent for minutes on a slow disk: keep the link alive, never time out.
        inactivity_timeout: None,
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 4,
        preferred,
        ..Default::default()
    });
    let connected = tokio::time::timeout(
        CONNECT_TIMEOUT,
        client::connect(config, (host, port), handler),
    )
    .await;
    match connected {
        Err(_) => Err(IntegrationError::retryable(format!(
            "SSH connection to {host}:{port} timed out"
        ))),
        Ok(Ok(handle)) => {
            let key = seen.lock().unwrap().clone().ok_or_else(|| {
                IntegrationError::fatal(format!("{host} accepted without a host key check"))
            })?;
            Ok(Handshake::Accepted(handle, key))
        }
        Ok(Err(e)) => {
            if let Some(p) = problem.lock().unwrap().clone() {
                return Ok(Handshake::Rejected(p));
            }
            // Only the recorded types were offered and the server has none of them any more.
            if let (
                russh::Error::NoCommonAlgo {
                    kind: russh::AlgorithmKind::Key,
                    ..
                },
                Some(line),
            ) = (&e, first_line)
            {
                return Ok(Handshake::Rejected(HostKeyProblem::Changed {
                    line,
                    key: None,
                }));
            }
            Err(IntegrationError::retryable(format!(
                "SSH connection to {host}:{port} failed: {e}"
            )))
        }
    }
}

/// Dropping it closes the connection.
pub struct SshShell {
    handle: client::Handle<KnownHostsCheck>,
}

impl SshShell {
    /// Signs in with the target's key file, else the SSH agent.
    pub async fn connect(target: &SshTarget) -> Result<Self> {
        Self::connect_auth(target, Auth::Profile).await
    }

    pub async fn connect_auth(target: &SshTarget, auth: Auth) -> Result<Self> {
        let mut handle = Self::open(target).await?;
        authenticate(&mut handle, target, auth).await?;
        Ok(Self { handle })
    }

    /// The connection, the server's host key checked against `known_hosts`, not signed in yet.
    async fn open(target: &SshTarget) -> Result<client::Handle<KnownHostsCheck>> {
        let file = known_hosts::resolve(target.known_hosts_file.as_deref())?;
        match handshake(&target.host, target.port, &file).await? {
            Handshake::Accepted(handle, _) => Ok(handle),
            Handshake::Rejected(p) => Err(host_key_error(&target.host, &file, &p)),
        }
    }
}

fn host_key_error(host: &str, known_hosts: &Path, problem: &HostKeyProblem) -> IntegrationError {
    let file = known_hosts.display();
    IntegrationError::fatal(match problem {
        HostKeyProblem::Unknown { key } => format!(
            "host key of {host} is not in {file} ({}); check its fingerprint and trust it in the \
             destination's setup, or connect once with `ssh`",
            known_hosts::fingerprint(key)
        ),
        HostKeyProblem::Changed { line, key } => {
            let offered = match key {
                Some(k) => format!("it now presents {}", known_hosts::fingerprint(k)),
                None => "it no longer offers a key of the type on record".into(),
            };
            format!(
                "host key of {host} does not match {file} line {line} ({offered}): the server \
                 changed or the connection is being intercepted"
            )
        }
        HostKeyProblem::Certificate => {
            format!("{host} offered a host certificate; only plain host keys are supported")
        }
    })
}

/// Where a server's host key stands in the `known_hosts` file filebridge uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKeyScan {
    /// The file checked, and the one [`trust_host_key`] adds to.
    pub known_hosts: PathBuf,
    pub standing: Standing,
    /// The key the server presented; `None` only when it is [`Standing::Changed`] and the server
    /// offered none of the key types on record.
    pub key: Option<PublicKey>,
}

/// Connects to `host:port` only far enough to see its host key (it never signs in) and says
/// whether `known_hosts_file` (default `~/.ssh/known_hosts`) holds it, holds nothing for the
/// host, or holds another key for it. `Err` when the server can't be reached, offers a
/// certificate, or the file can't be read.
pub async fn scan_host_key(
    host: &str,
    port: u16,
    known_hosts_file: Option<&Path>,
) -> Result<HostKeyScan> {
    let known_hosts = known_hosts::resolve(known_hosts_file)?;
    let (standing, key) = match handshake(host, port, &known_hosts).await? {
        Handshake::Accepted(_, key) => (Standing::Known, Some(key)),
        Handshake::Rejected(HostKeyProblem::Unknown { key }) => (Standing::Unknown, Some(key)),
        Handshake::Rejected(HostKeyProblem::Changed { line, key }) => {
            (Standing::Changed { line }, key)
        }
        Handshake::Rejected(p @ HostKeyProblem::Certificate) => {
            return Err(host_key_error(host, &known_hosts, &p));
        }
    };
    Ok(HostKeyScan {
        known_hosts,
        standing,
        key,
    })
}

/// Why [`trust_host_key`] added nothing.
#[derive(Debug, Clone)]
pub enum TrustError {
    /// The server's key is not one to trust: another is on record for the host, or the server
    /// presents another key than the one the user confirmed.
    Refused(String),
    /// The server couldn't be reached, or `known_hosts` couldn't be read or written.
    Failed(IntegrationError),
}

impl std::fmt::Display for TrustError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(m) => f.write_str(m),
            Self::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl From<IntegrationError> for TrustError {
    fn from(e: IntegrationError) -> Self {
        Self::Failed(e)
    }
}

/// One trust at a time in this process, so two never both see the host as unknown and append.
static TRUSTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Adds the key `host:port` presents to `known_hosts_file`, after the user checked that its
/// fingerprint is `fingerprint` (`SHA256:…`). Only for a host with no key there yet: a host
/// whose key changed is refused, and so is a server that now presents another key than the one
/// confirmed. Answers the scan after the change (`Known`).
pub async fn trust_host_key(
    host: &str,
    port: u16,
    known_hosts_file: Option<&Path>,
    fingerprint: &str,
) -> std::result::Result<HostKeyScan, TrustError> {
    let _one = TRUSTING.lock().await;
    let scan = scan_host_key(host, port, known_hosts_file).await?;
    let key = match (&scan.standing, &scan.key) {
        (Standing::Known, _) => return Ok(scan),
        (Standing::Changed { line }, key) => {
            let p = HostKeyProblem::Changed {
                line: *line,
                key: key.clone(),
            };
            return Err(TrustError::Refused(
                host_key_error(host, &scan.known_hosts, &p).message,
            ));
        }
        (Standing::Unknown, Some(key)) => key.clone(),
        (Standing::Unknown, None) => unreachable!("an unknown host was seen with its key"),
    };
    let presented = known_hosts::fingerprint(&key);
    if presented != fingerprint.trim() {
        return Err(TrustError::Refused(format!(
            "{host} now presents the key {presented}, not the one you checked ({}); nothing was \
             trusted: check the server again",
            fingerprint.trim()
        )));
    }
    let (h, file, k) = (host.to_owned(), scan.known_hosts.clone(), key.clone());
    tokio::task::spawn_blocking(move || known_hosts::append(&h, port, &k, &file))
        .await
        .map_err(|e| IntegrationError::fatal(format!("adding the host key stopped: {e}")))??;
    Ok(HostKeyScan {
        standing: Standing::Known,
        key: Some(key),
        ..scan
    })
}

async fn authenticate(
    handle: &mut client::Handle<KnownHostsCheck>,
    target: &SshTarget,
    auth: Auth,
) -> Result<()> {
    let user = target.user.as_str();
    let net = |e: russh::Error| IntegrationError::retryable(format!("SSH authentication: {e}"));
    let rsa_hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(net)?
        .flatten();

    if let Auth::Key(key) = auth {
        let res = handle
            .authenticate_publickey(user, PrivateKeyWithHashAlg::new(key, rsa_hash))
            .await
            .map_err(net)?;
        return if res.success() {
            Ok(())
        } else if key_works(&res) {
            Err(IntegrationError::fatal(format!(
                "server took filebridge's own key for user {user} but asks for another sign-in \
                 step after it (AuthenticationMethods), which filebridge can't answer"
            )))
        } else {
            Err(IntegrationError::fatal(format!(
                "server refused filebridge's own key for user {user}; it may have been removed \
                 there (Remove access, or by hand)"
            )))
        };
    }

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
    fn a_key_taken_as_a_first_factor_works() {
        let failure = |partial_success| client::AuthResult::Failure {
            remaining_methods: russh::MethodSet::empty(),
            partial_success,
        };
        assert!(key_works(&client::AuthResult::Success));
        assert!(key_works(&failure(true)));
        assert!(!key_works(&failure(false)));
    }

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
