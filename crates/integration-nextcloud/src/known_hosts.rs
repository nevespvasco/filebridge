//! The `known_hosts` file filebridge checks servers' host keys against, and the one way the app
//! adds to it: the user trusting the key of a server that has no key there yet, after seeing its
//! type and SHA-256 fingerprint (ADR 0015). A host that already has a key on record is never
//! given another one here: a different key stays an error.
//!
//! The file is only ever appended to, so every line already in it — hashed (`|1|…`), commented,
//! `@cert-authority`, other hosts' — stays exactly as it was.

use std::borrow::Cow;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use filebridge_integration_api::{IntegrationError, Result};
use russh::keys::{Algorithm, HashAlg, PublicKey};

/// `$HOME/.ssh/known_hosts` (on a phone, `HOME` is the app's own folder).
pub fn default_path() -> Result<PathBuf> {
    std::env::home_dir()
        .map(|h| h.join(".ssh").join("known_hosts"))
        .ok_or_else(|| IntegrationError::fatal("no home directory for known_hosts"))
}

/// The profile's file, else [`default_path`].
pub fn resolve(file: Option<&Path>) -> Result<PathBuf> {
    match file {
        Some(p) => Ok(p.to_path_buf()),
        None => default_path(),
    }
}

/// The keys `path` holds for `host:port`, with their line numbers (1-based). A missing file holds
/// none.
pub fn recorded(host: &str, port: u16, path: &Path) -> Result<Vec<(usize, PublicKey)>> {
    let unreadable = |e: &dyn std::fmt::Display| {
        IntegrationError::fatal(format!("could not check {}: {e}", path.display()))
    };
    let found = russh::keys::known_hosts::known_host_keys_path(host, port, path)
        .map_err(|e| unreadable(&e))?;
    if found.is_empty() {
        return Ok(found);
    }
    // russh numbers only the lines that aren't comments: give the line as an editor shows it.
    let text = std::fs::read(path).map_err(|e| unreadable(&e))?;
    let counted: Vec<usize> = text
        .split(|&b| b == b'\n')
        .enumerate()
        .filter(|(_, l)| l.first() != Some(&b'#'))
        .map(|(i, _)| i + 1)
        .collect();
    Ok(found
        .into_iter()
        .map(|(n, key)| (counted.get(n - 1).copied().unwrap_or(n), key))
        .collect())
}

/// Where a server's key stands in `known_hosts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standing {
    /// The file holds this key for the host.
    Known,
    /// The file holds no key at all for the host.
    Unknown,
    /// The file holds another key for the host (of this type or another), first at `line`.
    Changed { line: usize },
}

/// Where `key` stands among `recorded`. Any recorded key that isn't this one makes it
/// [`Standing::Changed`], whatever its type: the connection only offers the recorded types
/// ([`host_key_algorithms`]), so a key of another type means the server changed.
pub fn standing(recorded: &[(usize, PublicKey)], key: &PublicKey) -> Standing {
    if recorded.iter().any(|(_, k)| k.key_data() == key.key_data()) {
        return Standing::Known;
    }
    let same_type = recorded
        .iter()
        .find(|(_, k)| k.algorithm() == key.algorithm());
    match same_type.or(recorded.first()) {
        Some((line, _)) => Standing::Changed { line: *line },
        None => Standing::Unknown,
    }
}

/// The host key algorithms to offer the server: only the types `recorded` holds, so a server with
/// several keys presents the one on record (as `ssh` does). Empty when nothing is recorded: then
/// any type will do.
pub fn host_key_algorithms(recorded: &[(usize, PublicKey)]) -> Vec<Algorithm> {
    let mut out: Vec<Algorithm> = Vec::new();
    for (_, key) in recorded {
        let algos = match key.algorithm() {
            // An RSA key signs with SHA-2 on any current server; ssh-rsa (SHA-1) last.
            Algorithm::Rsa { .. } => vec![
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha512),
                },
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha256),
                },
                Algorithm::Rsa { hash: None },
            ],
            other => vec![other],
        };
        for a in algos {
            if !out.contains(&a) {
                out.push(a);
            }
        }
    }
    out
}

/// `SHA256:…`, as `ssh` and `ssh-keygen -l` show it.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// The host field of a new line: `host`, or `[host]:port` off port 22 (what `ssh` writes and
/// what the checks look up). Only names and addresses are accepted: a `,`, space, wildcard or
/// leading `|`/`@`/`!` would make the line trust other hosts too, or none.
fn host_field(host: &str, port: u16) -> Result<Cow<'_, str>> {
    let ok = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b':'))
        && !host.starts_with(['-', '.']);
    if !ok {
        return Err(IntegrationError::fatal(format!(
            "{host:?} is not a host name or address filebridge can add to known_hosts"
        )));
    }
    Ok(if port == 22 {
        Cow::Borrowed(host)
    } else {
        Cow::Owned(format!("[{host}]:{port}"))
    })
}

/// Adds `key` for `host:port` at the end of `path`, unless the host already has a key there:
/// the same key is a no-op, another one is refused. Creates the file (mode 600) and its folder
/// (700) when missing, and first ends a last line that lacks its newline. Never rewrites what is
/// already in the file. Blocking.
pub fn append(host: &str, port: u16, key: &PublicKey, path: &Path) -> Result<()> {
    let field = host_field(host, port)?;
    if !path.is_absolute() {
        return Err(IntegrationError::fatal(format!(
            "known_hosts file {} must be a full path",
            path.display()
        )));
    }
    let io = |what: &str, e: std::io::Error| {
        IntegrationError::fatal(format!("could not {what} {}: {e}", path.display()))
    };
    // Checked again here, right before writing: the file may have changed since the scan.
    match standing(&recorded(host, port, path)?, key) {
        Standing::Known => return Ok(()),
        Standing::Changed { line } => {
            return Err(IntegrationError::fatal(format!(
                "{} line {line} already has another key for {host}; filebridge never replaces \
                 a host key",
                path.display()
            )));
        }
        Standing::Unknown => {}
    }
    if let Some(dir) = path.parent() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder
            .create(dir)
            .map_err(|e| io("create the folder of", e))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).append(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).map_err(|e| io("open", e))?;
    let mut line = String::new();
    if file.seek(SeekFrom::End(-1)).is_ok() {
        let mut last = [0u8; 1];
        file.read_exact(&mut last).map_err(|e| io("read", e))?;
        if last[0] != b'\n' {
            line.push('\n');
        }
    }
    let public = key
        .to_openssh()
        .map_err(|e| IntegrationError::fatal(format!("encode the host key: {e}")))?;
    // The key without the server's comment, if it sent one.
    let mut words = public.split_whitespace();
    let (Some(kind), Some(data)) = (words.next(), words.next()) else {
        return Err(IntegrationError::fatal("encode the host key: empty"));
    };
    line.push_str(&format!("{field} {kind} {data}\n"));
    file.write_all(line.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|e| io("write", e))?;
    match standing(&recorded(host, port, path)?, key) {
        Standing::Known => Ok(()),
        _ => Err(IntegrationError::fatal(format!(
            "the key was written to {} but is not found there for {host}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
    const KEY_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIA6rWI3G1sz07DnfFlrouTcysQlj2P+jpNSOEWD9OJ3X";
    const KEY_C: &str = "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";

    fn key(b64: &str) -> PublicKey {
        russh::keys::parse_public_key_base64(b64).unwrap()
    }

    /// `example.com` hashed (`ssh-keygen -H`), with [`KEY_C`].
    const HASHED_EXAMPLE_COM: &str = "|1|O33ESRMWPVkMYIwJ1Uw+n877jTo=|nuuC5vEqXlEZ/8BXQR7m619W6Ak= \
                                      ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";
    const ECDSA: &str = "AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBHRsrpdHlLPGsqF9evhnbKcz\
                         Mh2haf5gLIgWJ6fodaeB1oCfEQMsvpOKUFeaI0ih2ww0uai37t7MRXbDTFAH8aw=";
    const RSA: &str = "AAAAB3NzaC1yc2EAAAADAQABAAAAgQDNCjZrku7BtZZ9diWifu5wE3iWUxgAwiwAHsvEaN3qlJep\
                       jFzJy+A2ZP2Q4SD+5rQ/3J4GuRY7AsKejJaJ6VMk85dFrr8c0bZBING+nvxc78bjSMXR86zHc41I\
                       nh7FoKRBvKmYQS0tHFI9F7BxfFlbDhaW42Wa4ZuAzoUIcZr8oQ==";

    #[test]
    fn appends_after_existing_lines_and_leaves_them_intact() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        // Another host hashed, a comment, a plain line, and no newline at the end.
        let before =
            format!("{HASHED_EXAMPLE_COM}\n# kept as is\nother.example ssh-ed25519 {KEY_B}");
        std::fs::write(&path, &before).unwrap();

        append("nas.example", 22, &key(KEY_A), &path).unwrap();
        append("nas.example", 2222, &key(KEY_A), &path).unwrap();
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            after,
            format!(
                "{before}\nnas.example ssh-ed25519 {KEY_A}\n[nas.example]:2222 ssh-ed25519 \
                 {KEY_A}\n"
            )
        );
        for port in [22, 2222] {
            let found = recorded("nas.example", port, &path).unwrap();
            assert_eq!(standing(&found, &key(KEY_A)), Standing::Known);
        }
        // The hashed line still matches its host, with its own key.
        let hidden = recorded("example.com", 22, &path).unwrap();
        assert_eq!(standing(&hidden, &key(KEY_C)), Standing::Known);
        // Trusting the same key again changes nothing.
        append("nas.example", 22, &key(KEY_A), &path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), after);
    }

    #[test]
    fn a_host_with_another_key_is_refused_also_when_hashed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        let before = format!("# first\n{HASHED_EXAMPLE_COM}\n");
        std::fs::write(&path, &before).unwrap();
        let found = recorded("example.com", 22, &path).unwrap();
        assert_eq!(standing(&found, &key(KEY_A)), Standing::Changed { line: 2 });
        let e = append("example.com", 22, &key(KEY_A), &path).unwrap_err();
        assert!(e.message.contains("line 2 already has another key"), "{e}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn a_key_of_another_type_on_record_counts_as_changed() {
        let found = vec![(3, key(ECDSA)), (5, key(RSA))];
        assert_eq!(standing(&found, &key(KEY_A)), Standing::Changed { line: 3 });
        assert_eq!(standing(&found, &key(RSA)), Standing::Known);
        assert_eq!(
            host_key_algorithms(&found),
            [
                Algorithm::Ecdsa {
                    curve: russh::keys::EcdsaCurve::NistP256
                },
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha512)
                },
                Algorithm::Rsa {
                    hash: Some(HashAlg::Sha256)
                },
                Algorithm::Rsa { hash: None },
            ]
        );
        assert!(host_key_algorithms(&[]).is_empty());
    }

    #[test]
    fn creates_a_private_file_and_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("home/.ssh/known_hosts");
        append("192.0.2.10", 22, &key(KEY_A), &path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("192.0.2.10 ssh-ed25519 {KEY_A}\n")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
    }

    #[test]
    fn refuses_hosts_that_would_widen_the_line_and_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        for host in [
            "a.example,b.example",
            "*.example",
            "a.example b.example",
            "|1|x",
            "@cert-authority",
            "!a.example",
            "",
        ] {
            assert!(append(host, 22, &key(KEY_A), &path).is_err(), "{host}");
        }
        assert!(!path.exists());
        let e = append(
            "a.example",
            22,
            &key(KEY_A),
            Path::new("~/.ssh/known_hosts"),
        )
        .unwrap_err();
        assert!(e.message.contains("full path"), "{e}");
        // IPv6 addresses are fine.
        append("2001:db8::10", 2222, &key(KEY_A), &path).unwrap();
        let found = recorded("2001:db8::10", 2222, &path).unwrap();
        assert_eq!(standing(&found, &key(KEY_A)), Standing::Known);
    }
}
