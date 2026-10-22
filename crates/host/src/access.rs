//! filebridge's own SSH key for a profile, and "Remove access" (ADR 0012, docs/07).
//!
//! Installing: an Ed25519 key is generated, its public line added with `restrict` to the
//! `authorized_keys` of each SSH user the profile signs in as (through the access it has today),
//! the private key stored in the OS keyring, and only once a sign-in with the new key has worked
//! (and rclone has listed the destination with it) is the profile switched to it. Any failure
//! undoes what was done, and the report says what the undo did for each server.
//!
//! Removing: the line is deleted from each server (signed in with the key itself), a last
//! sign-in with the old key, held in memory, must be refused, and only then is the key deleted
//! from the keyring and the profile put back on its key file, SSH agent or password.

use std::sync::Arc;

use filebridge_core::{Engine, Profile};
use filebridge_integration_nextcloud::ssh::{NewKey, generate_key};
use filebridge_integration_nextcloud::{NextcloudSettings, ShellConnector, SshTarget};
use filebridge_ipc::{AccessReport, DetectStepView, ErrorBody};
use filebridge_secrets::{KeyringRef, SecretKind, SecretStore, reference};
use filebridge_transport_api::Backend;
use russh::keys::PrivateKey;

fn err(code: &str, message: impl Into<String>) -> ErrorBody {
    ErrorBody {
        code: code.into(),
        message: message.into(),
    }
}

fn step(id: &str, status: &str, message: impl Into<String>) -> DetectStepView {
    DetectStepView {
        id: id.into(),
        status: status.into(),
        message: message.into(),
        commands: Vec::new(),
    }
}

/// The comment that marks filebridge's line for one profile in `authorized_keys`.
fn comment(profile_id: &str) -> String {
    format!("filebridge-{profile_id}")
}

/// Profile ids go into shell scripts and a `grep` pattern: only `[A-Za-z0-9-]` (a UUID) is
/// accepted, so nothing in them is special to either.
fn check_id(profile_id: &str) -> Result<(), ErrorBody> {
    if !profile_id.is_empty()
        && profile_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        Ok(())
    } else {
        Err(err(
            "invalid",
            "a profile id is made of letters, digits and '-'",
        ))
    }
}

/// Adds `line` to `~/.ssh/authorized_keys` once, creating the folder and file private, and
/// first ending a last line that lacks its newline (or the new line would join it). Bytes are
/// compared as bytes (`LC_ALL=C grep -a`): a Latin-1 comment or a stray NUL elsewhere in the
/// file changes nothing.
pub(crate) fn install_script(line: &str) -> String {
    const SCRIPT: &str = r#"umask 077
mkdir -p ~/.ssh && f=~/.ssh/authorized_keys && touch "$f" || exit 1
l=@LINE@
LC_ALL=C grep -a -qxF -e "$l" "$f"; s=$?
[ "$s" -eq 0 ] && exit 0
[ "$s" -eq 1 ] || { echo "could not read $f" >&2; exit 1; }
if [ -s "$f" ] && [ "$(tail -c 1 "$f" | wc -l)" -eq 0 ]; then echo >> "$f" || exit 1; fi
printf '%s\n' "$l" >> "$f" || exit 1
LC_ALL=C grep -a -qxF -e "$l" "$f" || { echo "the line did not reach $f" >&2; exit 1; }
"#;
    SCRIPT.replace(
        "@LINE@",
        &filebridge_integration_nextcloud::quote::quote(line),
    )
}

/// The lines of filebridge's key for `profile_id`: the comment as the last word, also when the
/// line ends in `\r` (a file edited on Windows) or spaces.
fn pattern(profile_id: &str) -> String {
    format!("[[:space:]]{}[[:space:]]*$", comment(profile_id))
}

/// Removes every line of filebridge's key for `profile_id` from `~/.ssh/authorized_keys`, and
/// nothing else. The file is left exactly as it was unless every step worked:
/// - lines are compared as bytes (`LC_ALL=C grep -a`), so a Latin-1 comment or a NUL byte
///   never makes `grep` drop lines or answer "binary file matches";
/// - any `grep` error (status > 1) stops it;
/// - the filtered copy is made next to the file (same disk, mode 600), and must hold exactly
///   the old lines minus the matched ones, and no matched line, before it is written back.
///
/// Written back in place (`cat >`), so the file keeps its owner and mode.
pub(crate) fn remove_script(profile_id: &str) -> String {
    const SCRIPT: &str = r#"f=~/.ssh/authorized_keys
[ -f "$f" ] || exit 0
p=@PATTERN@
num() { case "$1" in ''|*[!0-9]*) return 1;; esac; }
fail() { echo "$1; $f was left as it was" >&2; exit 1; }
all=$(LC_ALL=C grep -a -c '' "$f"); [ "$?" -le 1 ] && num "$all" || fail "could not read $f"
ours=$(LC_ALL=C grep -a -c -e "$p" "$f"); [ "$?" -le 1 ] && num "$ours" || fail "could not read $f"
[ "$ours" -eq 0 ] && exit 0
t=$(mktemp "$f.filebridge.XXXXXX") || fail "could not make a temporary file in ~/.ssh"
trap 'rm -f "$t"' EXIT
LC_ALL=C grep -a -v -e "$p" "$f" > "$t"; [ "$?" -le 1 ] || fail "could not filter $f"
left=$(LC_ALL=C grep -a -c '' "$t"); [ "$?" -le 1 ] && num "$left" || fail "could not check the filtered copy"
still=$(LC_ALL=C grep -a -c -e "$p" "$t"); [ "$?" -le 1 ] && num "$still" || fail "could not check the filtered copy"
[ "$still" -eq 0 ] && [ "$left" -eq $((all - ours)) ] || fail "the filtered copy has $left of $all lines, expected $((all - ours))"
cat "$t" > "$f" || { trap - EXIT; echo "could not rewrite $f; what it should hold is in $t" >&2; exit 1; }
"#;
    SCRIPT.replace(
        "@PATTERN@",
        &filebridge_integration_nextcloud::quote::quote(&pattern(profile_id)),
    )
}

/// The SSH sign-ins a profile makes: its SFTP user, and for Nextcloud's direct mode the SSH user
/// that runs `occ` when it differs.
fn targets(profile: &Profile) -> Result<Vec<SshTarget>, ErrorBody> {
    let Backend::Sftp {
        host,
        port,
        user,
        key_file,
        known_hosts_file,
        ..
    } = &profile.destination.backend
    else {
        return Err(err(
            "invalid",
            "only destinations reached over SSH (SFTP, Nextcloud direct mode) have an SSH key",
        ));
    };
    let user = user
        .clone()
        .ok_or_else(|| err("invalid", "the profile names no SSH user"))?;
    let mut out = vec![SshTarget {
        host: host.clone(),
        port: port.unwrap_or(22),
        user,
        key_file: key_file.clone(),
        known_hosts_file: known_hosts_file.clone(),
        key: None,
    }];
    if let Ok(NextcloudSettings::Direct(nc)) =
        serde_json::from_value::<NextcloudSettings>(profile.integration_settings.clone())
        && !out
            .iter()
            .any(|t| t.host == nc.ssh.host && t.port == nc.ssh.port && t.user == nc.ssh.user)
    {
        out.push(SshTarget {
            key: None,
            ..nc.ssh.clone()
        });
    }
    Ok(out)
}

/// An SFTP profile with a password and no key file signs in by password only. The install
/// connects with the key file or SSH agent (the password is rclone's alone), so it is refused
/// for those rather than failing halfway.
fn password_only(profile: &Profile) -> bool {
    matches!(
        profile.destination.backend,
        Backend::Sftp {
            password: Some(_),
            key_file: None,
            ..
        }
    )
}

/// What the profile signs in with once filebridge's key is gone.
fn own_access(profile: &Profile) -> &'static str {
    match &profile.destination.backend {
        Backend::Sftp {
            password: Some(_), ..
        } => "the destination signs in with its password again",
        _ => "the destination signs in with your key file or SSH agent again",
    }
}

/// The profile signing in with `key` (or, with `None`, with its key file or agent again).
fn with_key(profile: &Profile, key: Option<KeyringRef>) -> Result<Profile, ErrorBody> {
    let mut p = profile.clone();
    if let Backend::Sftp { key: slot, .. } = &mut p.destination.backend {
        *slot = key.clone();
    }
    if let Ok(NextcloudSettings::Direct(mut nc)) =
        serde_json::from_value::<NextcloudSettings>(p.integration_settings.clone())
    {
        nc.ssh.key = key;
        p.integration_settings = serde_json::to_value(NextcloudSettings::Direct(nc))
            .map_err(|e| err("internal", e.to_string()))?;
    }
    Ok(p)
}

async fn keyring<T: Send + 'static>(
    store: &Arc<dyn SecretStore>,
    f: impl FnOnce(&dyn SecretStore) -> filebridge_secrets::Result<T> + Send + 'static,
) -> Result<T, ErrorBody> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || f(store.as_ref()))
        .await
        .map_err(|e| err("internal", e.to_string()))?
        .map_err(|e| err("keyring", e.to_string()))
}

fn who(t: &SshTarget) -> String {
    format!("{}@{}", t.user, t.host)
}

/// The profile's sign-in must not change under a running transfer.
async fn ensure_idle(engine: &Engine, profile_id: &str) -> Result<(), ErrorBody> {
    let active = engine
        .active_jobs_of(profile_id)
        .await
        .map_err(|e| err("internal", e.to_string()))?;
    if active.is_empty() {
        Ok(())
    } else {
        Err(err(
            "busy",
            "a transfer to this destination is running: pause it or let it finish first",
        ))
    }
}

/// Runs `script` on `t`, through `shell`; a non-zero exit is an error with its message.
async fn run_script(
    shell: filebridge_integration_api::Result<
        Arc<dyn filebridge_integration_nextcloud::RemoteShell>,
    >,
    script: &str,
) -> Result<(), String> {
    let shell = shell.map_err(|e| e.message)?;
    let out = shell.run(script).await.map_err(|e| e.message)?;
    if out.success() {
        Ok(())
    } else {
        Err(out.short_error())
    }
}

/// What a failed install has to undo.
struct Undo<'a> {
    connector: &'a dyn ShellConnector,
    secrets: &'a Arc<dyn SecretStore>,
    key_ref: &'a KeyringRef,
    profile_id: &'a str,
    /// Servers whose `authorized_keys` got the line.
    installed: Vec<SshTarget>,
    /// Whether the private key reached the keyring.
    stored: bool,
}

impl Undo<'_> {
    /// Undoes the install with the access the profile had, and reports what it did for each
    /// server and the keyring: a failed undo is never reported as "nothing was changed".
    async fn run(self, mut steps: Vec<DetectStepView>) -> AccessReport {
        if self.installed.is_empty() && !self.stored {
            steps.push(step("undone", "ok", "nothing was changed"));
        }
        for t in &self.installed {
            let outcome = run_script(
                self.connector.connect(t).await,
                &remove_script(self.profile_id),
            )
            .await;
            steps.push(match outcome {
                Ok(()) => step(
                    "undone",
                    "ok",
                    format!("the line was removed again from {}'s authorized_keys", who(t)),
                ),
                Err(e) => step(
                    "undone",
                    "fail",
                    format!(
                        "{}: {e}. The line ending in {} is still in its ~/.ssh/authorized_keys: remove it by hand",
                        who(t),
                        comment(self.profile_id)
                    ),
                ),
            });
        }
        if self.stored {
            let r = self.key_ref.clone();
            steps.push(match keyring(self.secrets, move |s| s.delete(&r)).await {
                Ok(()) => step(
                    "undone",
                    "ok",
                    "the key was deleted from the system keyring",
                ),
                Err(e) => step(
                    "undone",
                    "fail",
                    format!("the key is still in the system keyring: {}", e.message),
                ),
            });
        }
        AccessReport {
            steps,
            profile: None,
        }
    }
}

/// Generates, installs and switches the profile to filebridge's own key.
pub(crate) async fn install(
    engine: &Engine,
    connector: &dyn ShellConnector,
    profile_id: &str,
) -> Result<AccessReport, ErrorBody> {
    check_id(profile_id)?;
    let profile = engine
        .get_profile(profile_id)
        .await
        .map_err(|e| err("not_found", e.to_string()))?;
    if matches!(
        profile.destination.backend,
        Backend::Sftp { key: Some(_), .. }
    ) {
        return Err(err(
            "invalid",
            "this destination already uses filebridge's own key",
        ));
    }
    if password_only(&profile) {
        return Err(err(
            "invalid",
            "this destination signs in with a password: filebridge's own key is installed with a key file or SSH agent",
        ));
    }
    let targets = targets(&profile)?;
    ensure_idle(engine, profile_id).await?;
    let secrets = engine
        .registry()
        .secrets()
        .map_err(|e| err("keyring", e.to_string()))?;
    let NewKey { key, pem, public } =
        generate_key(&comment(profile_id)).map_err(|e| err("internal", e.message))?;
    let line = format!("restrict {public}");
    let mut steps = vec![step(
        "generate",
        "ok",
        "a new Ed25519 key, made on this computer",
    )];
    let key_ref = reference(profile_id, SecretKind::SshKey);
    let mut undo = Undo {
        connector,
        secrets: &secrets,
        key_ref: &key_ref,
        profile_id,
        installed: Vec::new(),
        stored: false,
    };

    for t in &targets {
        match run_script(connector.connect(t).await, &install_script(&line)).await {
            Ok(()) => {
                steps.push(step(
                    "install",
                    "ok",
                    format!("added to {}'s authorized_keys, restricted", who(t)),
                ));
                undo.installed.push(t.clone());
            }
            Err(e) => {
                steps.push(step("install", "fail", format!("{}: {e}", who(t))));
                return Ok(undo.run(steps).await);
            }
        }
    }

    let (r, value) = (key_ref.clone(), pem.to_string());
    if let Err(e) = keyring(&secrets, move |s| s.set(&r, &value)).await {
        steps.push(step("keyring", "fail", e.message));
        return Ok(undo.run(steps).await);
    }
    undo.stored = true;
    steps.push(step(
        "keyring",
        "ok",
        "the private key is in the system keyring",
    ));

    for t in &targets {
        match connector.key_accepted(t, key.clone()).await {
            Ok(true) => steps.push(step(
                "sign-in",
                "ok",
                format!("{} signs in with the new key", who(t)),
            )),
            outcome => {
                let why = match outcome {
                    Err(e) => e.message,
                    _ => "the server refused it".into(),
                };
                steps.push(step("sign-in", "fail", format!("{}: {why}", who(t))));
                return Ok(undo.run(steps).await);
            }
        }
    }

    let updated = with_key(&profile, Some(key_ref.clone()))?;
    let transport = engine
        .registry()
        .transport(&updated.destination.transport)
        .map_err(|e| err("internal", e.to_string()))?;
    if let Err(e) = transport.list(&updated.destination, None, true).await {
        steps.push(step("transfer", "fail", e.message));
        return Ok(undo.run(steps).await);
    }
    steps.push(step("transfer", "ok", "files can be sent with the new key"));

    let saved = match engine.save_profile(updated).await {
        Ok(saved) => saved,
        Err(e) => {
            steps.push(step("saved", "fail", e.to_string()));
            return Ok(undo.run(steps).await);
        }
    };
    steps.push(step(
        "saved",
        "ok",
        "the destination now uses filebridge's own key; your own key is no longer used",
    ));
    Ok(AccessReport {
        steps,
        profile: Some(saved),
    })
}

/// Deletes filebridge's key from the servers, proves it no longer works, and only then deletes
/// it from the keyring and puts the profile back on its own access.
pub(crate) async fn remove(
    engine: &Engine,
    connector: &dyn ShellConnector,
    profile_id: &str,
) -> Result<AccessReport, ErrorBody> {
    check_id(profile_id)?;
    let profile = engine
        .get_profile(profile_id)
        .await
        .map_err(|e| err("not_found", e.to_string()))?;
    let Backend::Sftp {
        key: Some(key_ref), ..
    } = &profile.destination.backend
    else {
        return Err(err(
            "invalid",
            "this destination doesn't use filebridge's own key",
        ));
    };
    let key_ref = key_ref.clone();
    let targets = targets(&profile)?;
    ensure_idle(engine, profile_id).await?;
    let secrets = engine
        .registry()
        .secrets()
        .map_err(|e| err("keyring", e.to_string()))?;
    let mut steps = Vec::new();
    // Held in memory for the proof.
    let key: Option<Arc<PrivateKey>> = {
        let r = key_ref.clone();
        keyring(&secrets, move |s| s.get(&r))
            .await
            .ok()
            .and_then(|pem| russh::keys::decode_secret_key(pem.0.as_str(), None).ok())
            .map(Arc::new)
    };
    if key.is_none() {
        steps.push(step(
            "keyring",
            "warn",
            "the key was not in the keyring any more; removing it from the servers with your own access",
        ));
    }

    for t in &targets {
        // With the key itself; if it no longer signs in, with the profile's own access.
        let shell = match &key {
            Some(k) => match connector.connect_with_key(t, k.clone()).await {
                Ok(s) => Ok(s),
                Err(_) => connector.connect(t).await,
            },
            None => connector.connect(t).await,
        };
        match run_script(shell, &remove_script(profile_id)).await {
            Ok(()) => steps.push(step(
                "remove",
                "ok",
                format!("removed from {}'s authorized_keys", who(t)),
            )),
            Err(e) => {
                steps.push(step("remove", "fail", format!("{}: {e}", who(t))));
                return Ok(AccessReport {
                    steps,
                    profile: None,
                });
            }
        }
    }

    // The proof, while the key is still kept: if a server still accepts it, the profile stays
    // on it (it still works there) and nothing else changes.
    if let Some(k) = &key {
        let mut accepted = false;
        for t in &targets {
            steps.push(match connector.key_accepted(t, k.clone()).await {
                Ok(false) => step(
                    "proof",
                    "ok",
                    format!("{} refuses the old key: access is gone", who(t)),
                ),
                Ok(true) => {
                    accepted = true;
                    step(
                        "proof",
                        "fail",
                        format!(
                            "{} still accepts the key: check its other authorized_keys files. The destination keeps using it",
                            who(t)
                        ),
                    )
                }
                Err(e) => step(
                    "proof",
                    "warn",
                    format!("could not check {}: {}", who(t), e.message),
                ),
            });
        }
        if accepted {
            return Ok(AccessReport {
                steps,
                profile: None,
            });
        }
    }

    // Saved first: a profile must never point at a key the keyring no longer has.
    let saved = match engine.save_profile(with_key(&profile, None)?).await {
        Ok(saved) => saved,
        Err(e) => {
            steps.push(step("saved", "fail", e.to_string()));
            return Ok(AccessReport {
                steps,
                profile: None,
            });
        }
    };
    steps.push(step("saved", "ok", own_access(&profile)));
    let r = key_ref.clone();
    steps.push(match keyring(&secrets, move |s| s.delete(&r)).await {
        Ok(()) => step("keyring", "ok", "deleted from the system keyring"),
        Err(e) => step(
            "keyring",
            "warn",
            format!(
                "it no longer signs in, but could not be deleted from the system keyring: {}",
                e.message
            ),
        ),
    });
    Ok(AccessReport {
        steps,
        profile: Some(saved),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Runs a script as `sh` would on the server, with `home` as `~`, in a UTF-8 locale (where
    /// GNU grep treats invalid UTF-8 and NUL bytes as "binary"), and `path` first on `PATH`.
    fn run_in(script: &str, home: &Path, path: Option<&Path>) -> bool {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script])
            .env("HOME", home)
            .env("LANG", "C.UTF-8")
            .env_remove("LC_ALL")
            .env_remove("LC_CTYPE");
        if let Some(dir) = path {
            let old = std::env::var_os("PATH").unwrap_or_default();
            let mut paths = vec![dir.to_path_buf()];
            paths.extend(std::env::split_paths(&old));
            cmd.env("PATH", std::env::join_paths(paths).unwrap());
        }
        cmd.status().unwrap().success()
    }

    fn run(script: &str, home: &Path) -> bool {
        run_in(script, home, None)
    }

    /// `home/.ssh/authorized_keys` holding `content`, mode 600.
    fn keys_file(home: &Path, content: &[u8]) -> PathBuf {
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let keys = ssh.join("authorized_keys");
        std::fs::write(&keys, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&keys, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        keys
    }

    /// A `grep` placed first on `PATH`: `body` runs for every call (`$REAL` is the real grep).
    fn fake_grep(body: &str) -> tempfile::TempDir {
        let real = String::from_utf8(
            Command::new("sh")
                .args(["-c", "command -v grep"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("grep");
        std::fs::write(&bin, format!("#!/bin/sh\nREAL={}\n{body}\n", real.trim())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        dir
    }

    #[test]
    fn installing_twice_adds_one_line_and_keeps_the_others() {
        let home = tempfile::tempdir().unwrap();
        // An existing key whose line has no final newline.
        let keys = keys_file(home.path(), b"ssh-ed25519 AAAAmine alice@laptop");
        let line = "restrict ssh-ed25519 AAAAnew filebridge-p-1";
        assert!(run(&install_script(line), home.path()));
        assert!(run(&install_script(line), home.path()));
        let text = std::fs::read_to_string(keys).unwrap();
        assert_eq!(text, format!("ssh-ed25519 AAAAmine alice@laptop\n{line}\n"));
    }

    #[test]
    fn installing_next_to_latin1_and_nul_bytes_adds_one_line() {
        let home = tempfile::tempdir().unwrap();
        let before: &[u8] = b"ssh-rsa AAAAold jos\xe9@laptop\nssh-ed25519 AAAA\0odd\n";
        let keys = keys_file(home.path(), before);
        let line = "restrict ssh-ed25519 AAAAnew filebridge-p-1";
        assert!(run(&install_script(line), home.path()));
        assert!(run(&install_script(line), home.path()));
        let mut want = before.to_vec();
        want.extend_from_slice(format!("{line}\n").as_bytes());
        assert_eq!(std::fs::read(keys).unwrap(), want);
    }

    #[test]
    fn installing_creates_a_private_ssh_folder() {
        let home = tempfile::tempdir().unwrap();
        assert!(run(
            &install_script("restrict ssh-ed25519 AAAA filebridge-p"),
            home.path()
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&home.path().join(".ssh")), 0o700);
            assert_eq!(mode(&home.path().join(".ssh/authorized_keys")), 0o600);
        }
    }

    #[test]
    fn removing_takes_only_that_profiles_lines() {
        let home = tempfile::tempdir().unwrap();
        let keys = keys_file(
            home.path(),
            b"ssh-ed25519 AAAAmine alice@laptop\n\
              restrict ssh-ed25519 AAAAone filebridge-p-1\n\
              restrict ssh-ed25519 AAAAtwo filebridge-p-12\n\
              ssh-rsa AAAAtheirs filebridge-p-1-notes and more\n",
        );
        assert!(run(&remove_script("p-1"), home.path()));
        let text = std::fs::read_to_string(&keys).unwrap();
        assert_eq!(
            text,
            "ssh-ed25519 AAAAmine alice@laptop\n\
             restrict ssh-ed25519 AAAAtwo filebridge-p-12\n\
             ssh-rsa AAAAtheirs filebridge-p-1-notes and more\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&keys).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        // Removing again, or with no file at all, is fine.
        assert!(run(&remove_script("p-1"), home.path()));
        let empty = tempfile::tempdir().unwrap();
        assert!(run(&remove_script("p-1"), empty.path()));
        // No temporary file is left behind.
        let left: Vec<_> = std::fs::read_dir(home.path().join(".ssh"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["authorized_keys"]);
    }

    #[test]
    fn removing_keeps_latin1_and_nul_lines_byte_for_byte() {
        let home = tempfile::tempdir().unwrap();
        // GNU grep in a UTF-8 locale would drop the Latin-1 line, or answer "binary file
        // matches" for the NUL, and empty the file.
        let keys = keys_file(
            home.path(),
            b"ssh-rsa AAAAold jos\xe9@laptop\n\
              restrict ssh-ed25519 AAAAone filebridge-p-1\n\
              ssh-ed25519 AAAA\0odd\n\
              ssh-ed25519 AAAAlast bob@desk",
        );
        assert!(run(&remove_script("p-1"), home.path()));
        assert_eq!(
            std::fs::read(&keys).unwrap(),
            b"ssh-rsa AAAAold jos\xe9@laptop\nssh-ed25519 AAAA\0odd\nssh-ed25519 AAAAlast bob@desk\n"
        );
    }

    #[test]
    fn removing_finds_lines_ending_in_crlf_or_spaces() {
        let home = tempfile::tempdir().unwrap();
        let keys = keys_file(
            home.path(),
            b"ssh-ed25519 AAAAmine alice@laptop\r\n\
              restrict ssh-ed25519 AAAAone filebridge-p-1\r\n\
              restrict ssh-ed25519 AAAAone\tfilebridge-p-1  \n",
        );
        assert!(run(&remove_script("p-1"), home.path()));
        assert_eq!(
            std::fs::read(&keys).unwrap(),
            b"ssh-ed25519 AAAAmine alice@laptop\r\n"
        );
    }

    const WITH_OURS: &[u8] = b"ssh-ed25519 AAAAmine alice@laptop\n\
        restrict ssh-ed25519 AAAAone filebridge-p-1\n";

    #[test]
    fn a_failing_grep_leaves_the_file_alone() {
        let home = tempfile::tempdir().unwrap();
        let keys = keys_file(home.path(), WITH_OURS);
        // grep's "trouble" status, e.g. a full disk while writing the filtered copy.
        let bin =
            fake_grep(r#"case " $* " in *" -v "*) "$REAL" "$@"; exit 2;; esac; exec "$REAL" "$@""#);
        assert!(!run_in(
            &remove_script("p-1"),
            home.path(),
            Some(bin.path())
        ));
        assert_eq!(std::fs::read(&keys).unwrap(), WITH_OURS);
        // Nothing can be read at all.
        let bin = fake_grep("exit 2");
        assert!(!run_in(
            &remove_script("p-1"),
            home.path(),
            Some(bin.path())
        ));
        assert_eq!(std::fs::read(&keys).unwrap(), WITH_OURS);
        let left = std::fs::read_dir(home.path().join(".ssh")).unwrap().count();
        assert_eq!(left, 1, "the temporary file is removed");
    }

    #[test]
    fn a_grep_that_drops_lines_silently_leaves_the_file_alone() {
        let home = tempfile::tempdir().unwrap();
        let keys = keys_file(home.path(), WITH_OURS);
        // Filtering "succeeds" with nothing written: the line count check refuses it.
        let bin = fake_grep(r#"case " $* " in *" -v "*) exit 0;; esac; exec "$REAL" "$@""#);
        assert!(!run_in(
            &remove_script("p-1"),
            home.path(),
            Some(bin.path())
        ));
        assert_eq!(std::fs::read(&keys).unwrap(), WITH_OURS);
    }

    #[test]
    fn a_failing_grep_never_installs_twice_or_reports_success() {
        let home = tempfile::tempdir().unwrap();
        let keys = keys_file(home.path(), WITH_OURS);
        let bin = fake_grep("exit 2");
        assert!(!run_in(
            &install_script("restrict ssh-ed25519 AAAAnew filebridge-p-2"),
            home.path(),
            Some(bin.path())
        ));
        assert_eq!(std::fs::read(&keys).unwrap(), WITH_OURS);
    }

    #[test]
    fn profile_ids_are_checked_before_any_script() {
        assert!(check_id("0b8f6a3e-1c2d-4e5f-9a0b-1c2d3e4f5a6b").is_ok());
        for bad in ["", "p 1", "p;rm", "p$1", "p.1", "p/1", "p*"] {
            assert!(check_id(bad).is_err(), "{bad:?}");
        }
    }

    /// Pretend SSH servers: one home per user under a temp folder; commands run there with
    /// `sh`, and a key signs in when its line is in that home's `authorized_keys` or
    /// `authorized_keys2`. After `connect_limit` connections, `connect` fails.
    struct LocalServers {
        dir: tempfile::TempDir,
        connects: AtomicUsize,
        connect_limit: usize,
    }

    struct LocalShell(PathBuf);

    impl LocalServers {
        fn new() -> Self {
            Self {
                dir: tempfile::tempdir().unwrap(),
                connects: AtomicUsize::new(0),
                connect_limit: usize::MAX,
            }
        }

        fn home(&self, user: &str) -> PathBuf {
            let h = self.dir.path().join(user);
            std::fs::create_dir_all(&h).unwrap();
            h
        }

        fn authorized(&self, user: &str) -> String {
            std::fs::read_to_string(self.home(user).join(".ssh/authorized_keys"))
                .unwrap_or_default()
        }
    }

    #[async_trait::async_trait]
    impl filebridge_integration_nextcloud::RemoteShell for LocalShell {
        async fn run(
            &self,
            cmd: &str,
        ) -> filebridge_integration_api::Result<filebridge_integration_nextcloud::Output> {
            let out = Command::new("sh")
                .args(["-c", cmd])
                .env("HOME", &self.0)
                .output()
                .unwrap();
            Ok(filebridge_integration_nextcloud::Output {
                status: out.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
    }

    #[async_trait::async_trait]
    impl ShellConnector for LocalServers {
        async fn connect(
            &self,
            target: &SshTarget,
        ) -> filebridge_integration_api::Result<
            Arc<dyn filebridge_integration_nextcloud::RemoteShell>,
        > {
            if self.connects.fetch_add(1, Ordering::SeqCst) >= self.connect_limit {
                return Err(filebridge_integration_api::IntegrationError::fatal(
                    "connection refused",
                ));
            }
            Ok(Arc::new(LocalShell(self.home(&target.user))))
        }

        async fn connect_with_key(
            &self,
            target: &SshTarget,
            key: Arc<PrivateKey>,
        ) -> filebridge_integration_api::Result<
            Arc<dyn filebridge_integration_nextcloud::RemoteShell>,
        > {
            if self.key_accepted(target, key).await? {
                self.connect(target).await
            } else {
                Err(filebridge_integration_api::IntegrationError::fatal(
                    "refused",
                ))
            }
        }

        async fn key_accepted(
            &self,
            target: &SshTarget,
            key: Arc<PrivateKey>,
        ) -> filebridge_integration_api::Result<bool> {
            let public = key.public_key().to_openssh().unwrap();
            let base64 = public.split_whitespace().nth(1).unwrap().to_owned();
            let second =
                std::fs::read_to_string(self.home(&target.user).join(".ssh/authorized_keys2"))
                    .unwrap_or_default();
            Ok(self
                .authorized(&target.user)
                .lines()
                .chain(second.lines())
                .any(|l| l.split_whitespace().any(|w| w == base64)))
        }
    }

    fn sftp(user: &str) -> Backend {
        Backend::Sftp {
            host: "nas.example.lan".into(),
            port: None,
            user: Some(user.into()),
            key_file: None,
            known_hosts_file: None,
            password: None,
            key: None,
        }
    }

    async fn engine_with(
        root: &Path,
        backend: Backend,
        integration_settings: serde_json::Value,
    ) -> (Arc<Engine>, Arc<filebridge_secrets::MemoryStore>, String) {
        let secrets = Arc::new(filebridge_secrets::MemoryStore::default());
        let mut registry = filebridge_core::Registry::new();
        registry.add_transport(Arc::new(filebridge_transport_fake::FakeTransport::default()));
        registry.set_secrets(secrets.clone());
        let engine = Engine::new(
            filebridge_core::Store::open_in_memory().unwrap(),
            registry,
            filebridge_core::EngineOptions {
                local_clock: filebridge_core::LocalClock::fixed(|| 12 * 3600),
                ..Default::default()
            },
        );
        let p = engine
            .save_profile(Profile {
                id: String::new(),
                name: "NAS".into(),
                destination: filebridge_transport_api::Destination {
                    transport: "fake".into(),
                    backend,
                    root: root.to_string_lossy().into_owned(),
                },
                integration: None,
                integration_settings,
                target: filebridge_core::TargetSpec::default(),
            })
            .await
            .unwrap();
        (engine, secrets, p.id)
    }

    async fn engine_with_profile(
        root: &Path,
    ) -> (Arc<Engine>, Arc<filebridge_secrets::MemoryStore>, String) {
        engine_with(root, sftp("alice"), serde_json::json!({})).await
    }

    fn failures(report: &AccessReport) -> Vec<&DetectStepView> {
        report.steps.iter().filter(|s| s.status != "ok").collect()
    }

    #[tokio::test]
    async fn install_then_remove_access() {
        let dest = tempfile::tempdir().unwrap();
        let (engine, secrets, id) = engine_with_profile(dest.path()).await;
        let servers = LocalServers::new();
        // Someone else's key, which must survive both operations.
        keys_file(
            &servers.home("alice"),
            b"ssh-ed25519 AAAAmine alice@laptop\n",
        );

        let report = install(&engine, &servers, &id).await.unwrap();
        assert!(failures(&report).is_empty(), "{report:?}");
        let saved = report.profile.unwrap();
        let key_ref = reference(&id, SecretKind::SshKey);
        assert!(
            matches!(&saved.destination.backend, Backend::Sftp { key: Some(r), .. } if *r == key_ref)
        );
        let pem = secrets.get(&key_ref).unwrap();
        assert!(pem.0.contains("OPENSSH PRIVATE KEY"));
        let lines = servers.authorized("alice");
        assert!(lines.starts_with("ssh-ed25519 AAAAmine alice@laptop\n"));
        let ours = lines
            .lines()
            .find(|l| l.ends_with(&format!(" filebridge-{id}")))
            .unwrap();
        assert!(ours.starts_with("restrict ssh-ed25519 "), "{ours}");
        assert!(
            install(&engine, &servers, &id).await.is_err(),
            "a second key is refused"
        );

        let report = remove(&engine, &servers, &id).await.unwrap();
        assert!(failures(&report).is_empty(), "{report:?}");
        let proof = report.steps.iter().find(|s| s.id == "proof").unwrap();
        assert_eq!(proof.status, "ok", "{report:?}");
        assert_eq!(
            servers.authorized("alice"),
            "ssh-ed25519 AAAAmine alice@laptop\n"
        );
        assert!(secrets.get(&key_ref).is_err());
        let p = engine.get_profile(&id).await.unwrap();
        assert!(matches!(
            p.destination.backend,
            Backend::Sftp { key: None, .. }
        ));
    }

    #[tokio::test]
    async fn a_failed_install_leaves_nothing_behind() {
        // The destination folder doesn't exist: the last check (listing it) fails.
        let missing = tempfile::tempdir().unwrap().path().join("nope");
        let (engine, secrets, id) = engine_with_profile(&missing).await;
        let servers = LocalServers::new();
        let report = install(&engine, &servers, &id).await.unwrap();
        assert!(report.profile.is_none());
        assert!(
            report
                .steps
                .iter()
                .any(|s| s.id == "transfer" && s.status == "fail")
        );
        let undone: Vec<_> = report.steps.iter().filter(|s| s.id == "undone").collect();
        assert_eq!(undone.len(), 2, "the server and the keyring: {report:?}");
        assert!(undone.iter().all(|s| s.status == "ok"), "{report:?}");
        assert!(
            !servers.authorized("alice").contains("filebridge-"),
            "line removed again"
        );
        assert!(secrets.get(&reference(&id, SecretKind::SshKey)).is_err());
        let p = engine.get_profile(&id).await.unwrap();
        assert!(matches!(
            p.destination.backend,
            Backend::Sftp { key: None, .. }
        ));
    }

    #[tokio::test]
    async fn a_failed_undo_says_what_is_left() {
        let missing = tempfile::tempdir().unwrap().path().join("nope");
        let (engine, _secrets, id) = engine_with_profile(&missing).await;
        // The install connects once; the undo can't connect any more.
        let servers = LocalServers {
            connect_limit: 1,
            ..LocalServers::new()
        };
        let report = install(&engine, &servers, &id).await.unwrap();
        assert!(report.profile.is_none());
        let undo = report
            .steps
            .iter()
            .find(|s| s.id == "undone" && s.status == "fail")
            .unwrap_or_else(|| panic!("{report:?}"));
        assert!(undo.message.contains("alice@nas.example.lan"), "{undo:?}");
        assert!(
            undo.message.contains(&format!("filebridge-{id}")),
            "{undo:?}"
        );
        assert!(
            !report
                .steps
                .iter()
                .any(|s| s.message.contains("nothing was changed")),
            "{report:?}"
        );
        assert!(servers.authorized("alice").contains("filebridge-"));
    }

    #[tokio::test]
    async fn every_ssh_account_of_direct_mode_gets_the_key() {
        let dest = tempfile::tempdir().unwrap();
        let nc = serde_json::json!({
            "mode": "direct",
            "ssh": { "host": "pve.example.lan", "port": 22, "user": "root" },
            "host_sudo": false,
            "runtime": { "kind": "host" },
            "install_dir": "/var/www/nextcloud",
            "php": "php",
            "run_as": "sudo",
            "web_user": "www-data",
            "data_dir": "/srv/nextcloud/data",
            "host_data_dir": "/srv/nextcloud/data",
            "user": "alice",
            "owner": { "uid": 33, "gid": 33 },
            "dir_mode": "750",
            "file_mode": "640"
        });
        assert!(matches!(
            serde_json::from_value::<NextcloudSettings>(nc.clone()),
            Ok(NextcloudSettings::Direct(_))
        ));
        let (engine, _secrets, id) = engine_with(dest.path(), sftp("alice"), nc).await;
        let servers = LocalServers::new();
        let report = install(&engine, &servers, &id).await.unwrap();
        assert!(failures(&report).is_empty(), "{report:?}");
        assert!(
            servers
                .authorized("alice")
                .contains(&format!("filebridge-{id}"))
        );
        assert!(
            servers
                .authorized("root")
                .contains(&format!("filebridge-{id}"))
        );
    }

    #[tokio::test]
    async fn a_key_still_accepted_keeps_the_profile_on_it() {
        let dest = tempfile::tempdir().unwrap();
        let (engine, secrets, id) = engine_with_profile(dest.path()).await;
        let servers = LocalServers::new();
        install(&engine, &servers, &id)
            .await
            .unwrap()
            .profile
            .unwrap();
        // The same line in a second file the server also reads: removing ours isn't enough.
        let ssh = servers.home("alice").join(".ssh");
        std::fs::copy(ssh.join("authorized_keys"), ssh.join("authorized_keys2")).unwrap();

        let report = remove(&engine, &servers, &id).await.unwrap();
        assert!(report.profile.is_none());
        let proof = report.steps.iter().find(|s| s.id == "proof").unwrap();
        assert_eq!(proof.status, "fail", "{report:?}");
        let key_ref = reference(&id, SecretKind::SshKey);
        assert!(secrets.get(&key_ref).is_ok(), "the key is kept");
        let p = engine.get_profile(&id).await.unwrap();
        assert!(
            matches!(&p.destination.backend, Backend::Sftp { key: Some(r), .. } if *r == key_ref),
            "the profile keeps signing in with it"
        );
    }

    #[tokio::test]
    async fn password_only_profiles_are_refused() {
        let dest = tempfile::tempdir().unwrap();
        let Backend::Sftp {
            host,
            port,
            user,
            known_hosts_file,
            ..
        } = sftp("alice")
        else {
            unreachable!()
        };
        let backend = Backend::Sftp {
            host,
            port,
            user,
            key_file: None,
            known_hosts_file,
            password: Some(reference("x", SecretKind::AppPassword)),
            key: None,
        };
        let (engine, _secrets, id) = engine_with(dest.path(), backend, serde_json::json!({})).await;
        let servers = LocalServers::new();
        let e = install(&engine, &servers, &id).await.unwrap_err();
        assert!(e.message.contains("password"), "{e:?}");
        assert_eq!(servers.connects.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn nothing_changes_while_a_transfer_runs() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dest) = (dir.path().join("src"), dir.path().join("dest"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(src.join("a.txt"), b"a").unwrap();
        let (engine, _secrets, id) = engine_with_profile(&dest).await;
        // A send window closed at the tests' noon: the job waits, still running.
        engine
            .save_settings(filebridge_core::AppSettings {
                bandwidth_limit: None,
                send_window: Some(filebridge_core::SendWindow {
                    start_minute: 22 * 60,
                    end_minute: 7 * 60,
                }),
            })
            .await
            .unwrap();
        let job = engine
            .create_job(vec![src], &id, Default::default())
            .await
            .unwrap();
        engine.analyse(&job).await.unwrap();
        engine.start(&job).await.unwrap();
        let servers = LocalServers::new();
        let e = install(&engine, &servers, &id).await.unwrap_err();
        assert_eq!(e.code, "busy", "{e:?}");
        assert_eq!(servers.connects.load(Ordering::SeqCst), 0);
        engine.cancel(&job).await.unwrap();
        assert!(
            install(&engine, &servers, &id)
                .await
                .unwrap()
                .profile
                .is_some()
        );
    }
}
