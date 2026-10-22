//! Builds rclone fs strings. Backends are always on-the-fly connection strings
//! (`:sftp,host=…:path`), so no `rclone.conf` is ever written; secrets go in them only in the
//! form rclone takes (an obscured password), over the local authenticated rc API, and are
//! scrubbed from rclone's error messages ([`redact`]).

use std::path::Path;

use filebridge_secrets::Redacted;
use filebridge_transport_api::{Backend, Destination, Result, TransportError};

/// A destination's secrets, fetched from the keyring for one call, as rclone takes them.
#[derive(Debug, Default, Clone)]
pub struct Creds {
    /// The password, obscured by rclone (`core/obscure`): its `pass=` parameter.
    pub pass: Option<Redacted<String>>,
}

/// The connection-string parameters that carry secrets; their values never leave this crate.
const SECRET_PARAMS: [&str; 2] = ["pass", "key_pem"];

/// fs string for `sub` (relative, may be empty) under the destination root.
pub fn dest_fs(dest: &Destination, creds: &Creds, sub: &str) -> Result<String> {
    let path = join(&dest.root, sub);
    match &dest.backend {
        Backend::Local => Ok(format!(":local:{path}")),
        Backend::Sftp {
            host,
            port,
            user,
            key_file,
            known_hosts_file,
            password,
        } => {
            let mut params = vec![("host", host.clone())];
            if password.is_some() {
                let pass = creds.pass.as_ref().ok_or_else(|| {
                    TransportError::new(
                        filebridge_transport_api::ErrorKind::Auth,
                        "the SFTP password is not in the keyring; set it again",
                    )
                })?;
                params.push(("pass", pass.0.clone()));
            }
            if let Some(port) = port {
                params.push(("port", port.to_string()));
            }
            if let Some(user) = user {
                params.push(("user", user.clone()));
            }
            if let Some(key) = key_file {
                params.push(("key_file", utf8(key)?.to_owned()));
            }
            // Always verify host keys; rclone skips verification when this is unset.
            let known_hosts = match known_hosts_file {
                Some(p) => utf8(p)?.to_owned(),
                None => default_known_hosts()?,
            };
            if let Some(algos) = host_key_algorithms(host, port.unwrap_or(22), &known_hosts) {
                params.push(("host_key_algorithms", algos));
            }
            params.push(("known_hosts_file", known_hosts));
            let params: Vec<String> = params
                .into_iter()
                .map(|(k, v)| format!("{k}={}", quote(&v)))
                .collect();
            Ok(format!(":sftp,{}:{path}", params.join(",")))
        }
    }
}

/// `message` with the value of every secret connection-string parameter (`pass="…"`,
/// `key_pem=…`) replaced by `***`: rclone repeats the fs string in some errors.
pub fn redact(message: &str) -> String {
    let starts_param = |s: &str, i: usize| i == 0 || matches!(s.as_bytes()[i - 1], b',' | b':');
    let mut out = String::with_capacity(message.len());
    let mut rest = message;
    loop {
        // The first secret parameter that starts a parameter (not `bypass=`).
        let hit = SECRET_PARAMS
            .iter()
            .flat_map(|name| {
                rest.match_indices(&format!("{name}="))
                    .filter(|(i, _)| starts_param(rest, *i))
                    .map(|(i, key)| (i, key.len()))
                    .next()
            })
            .min();
        let Some((i, key_len)) = hit else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..i + key_len]);
        out.push_str("***");
        let value = &rest[i + key_len..];
        let end = match value.strip_prefix('"') {
            // Quoted: up to the closing quote that isn't doubled.
            Some(quoted) => {
                let b = quoted.as_bytes();
                let mut j = 0;
                while j < b.len() && !(b[j] == b'"' && b.get(j + 1) != Some(&b'"')) {
                    j += if b[j] == b'"' { 2 } else { 1 };
                }
                (j + 2).min(value.len())
            }
            None => value.find([',', ':', ' ', '"']).unwrap_or(value.len()),
        };
        rest = &value[end..];
    }
}

/// fs string for a local source directory.
pub fn local_fs(path: &Path) -> Result<String> {
    Ok(format!(":local:{}", utf8(path)?))
}

/// Joins a root and a relative sub-path with `/`.
pub fn join(root: &str, sub: &str) -> String {
    match (root, sub) {
        (r, "") => r.to_owned(),
        ("", s) => s.to_owned(),
        (r, s) => format!("{}/{s}", r.trim_end_matches('/')),
    }
}

/// Splits a root into (parent, last component). Fails for `""` and `"/"`.
pub fn split_root(root: &str) -> Result<(&str, &str)> {
    let trimmed = root.trim_end_matches('/');
    let (parent, base) = match trimmed.rsplit_once('/') {
        Some(("", base)) => ("/", base),
        Some((parent, base)) => (parent, base),
        None => ("", trimmed),
    };
    if base.is_empty() {
        return Err(TransportError::fatal(format!(
            "destination root {root:?} has no parent"
        )));
    }
    Ok((parent, base))
}

pub(crate) fn utf8(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        TransportError::fatal(format!(
            "non-UTF-8 path {}: needs the `encoding` rule (Phase 2)",
            path.display()
        ))
    })
}

fn default_known_hosts() -> Result<String> {
    let home = std::env::var("HOME")
        .map_err(|_| TransportError::fatal("HOME not set; pass known_hosts_file explicitly"))?;
    Ok(format!("{home}/.ssh/known_hosts"))
}

/// The host key algorithms rclone may negotiate: only the key types `known_hosts` already holds
/// for this host. Go's SSH client otherwise picks its own preferred type (ECDSA before Ed25519)
/// and reports "knownhosts: key mismatch" when `known_hosts` has the host under another type.
/// `None` when the host is not there, so rclone reports it as unknown.
fn host_key_algorithms(host: &str, port: u16, known_hosts: &str) -> Option<String> {
    let keys = russh::keys::known_hosts::known_host_keys_path(host, port, known_hosts).ok()?;
    let mut algos: Vec<String> = Vec::new();
    for (_, key) in &keys {
        let algorithm = key.algorithm();
        let names: &[&str] = match algorithm.as_str() {
            // An RSA key signs with SHA-2 on any current server; ssh-rsa (SHA-1) last.
            "ssh-rsa" => &["rsa-sha2-512", "rsa-sha2-256", "ssh-rsa"],
            name => &[name],
        };
        for name in names {
            if !algos.iter().any(|a| a == name) {
                algos.push((*name).to_owned());
            }
        }
    }
    (!algos.is_empty()).then(|| algos.join(" "))
}

/// Connection-string value quoting: wrap in `"` and double any `"` inside.
fn quote(v: &str) -> String {
    format!("\"{}\"", v.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn sftp_connection_string() {
        let dest = Destination {
            transport: "rclone".into(),
            backend: Backend::Sftp {
                host: "nas.lan".into(),
                port: Some(2222),
                user: Some("ana".into()),
                key_file: Some(PathBuf::from("/k/id,ed25519")),
                known_hosts_file: Some(PathBuf::from("/k/known_hosts")),
                password: None,
            },
            root: "/srv/files/".into(),
        };
        assert_eq!(
            dest_fs(&dest, &Creds::default(), "Backup").unwrap(),
            ":sftp,host=\"nas.lan\",port=\"2222\",user=\"ana\",key_file=\"/k/id,ed25519\",\
             known_hosts_file=\"/k/known_hosts\":/srv/files/Backup"
        );
    }

    #[test]
    fn secrets_are_scrubbed_from_messages() {
        let msg = r#"didn't find section in config file: ":sftp,host="nas.lan",pass="AbC"",x",user="ana":/srv""#;
        let out = redact(msg);
        assert!(!out.contains("AbC"), "{out}");
        assert!(
            out.contains("pass=***") && out.contains("user=\"ana\""),
            "{out}"
        );
        assert_eq!(redact("key_pem=-----BEGIN,host=x"), "key_pem=***,host=x");
        assert_eq!(
            redact("bypass=1 pass= ok"),
            "bypass=1 pass= ok",
            "only whole parameters"
        );
        assert_eq!(redact("bypass=1,pass=s3cret:/x"), "bypass=1,pass=***:/x");
        assert_eq!(
            redact(":sftp,pass=a,key_pem=b:/"),
            ":sftp,pass=***,key_pem=***:/"
        );
        assert_eq!(redact("nothing here"), "nothing here");
    }

    #[test]
    fn a_password_profile_needs_its_secret() {
        let dest = Destination {
            transport: "rclone".into(),
            backend: Backend::Sftp {
                host: "nas.lan".into(),
                port: None,
                user: Some("ana".into()),
                key_file: None,
                known_hosts_file: Some(PathBuf::from("/k/known_hosts")),
                password: Some(filebridge_transport_api::KeyringRef(
                    "filebridge/p/app-password".into(),
                )),
            },
            root: "/srv".into(),
        };
        let err = dest_fs(&dest, &Creds::default(), "").unwrap_err();
        assert_eq!(err.kind, filebridge_transport_api::ErrorKind::Auth);
        let creds = Creds {
            pass: Some(Redacted("obscured".into())),
        };
        let fs = dest_fs(&dest, &creds, "").unwrap();
        assert!(fs.contains("pass=\"obscured\""), "{fs}");
    }

    #[test]
    fn host_key_algorithms_follow_known_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("known_hosts");
        std::fs::write(
            &file,
            "nas.lan ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ\n",
        )
        .unwrap();
        let file = file.to_str().unwrap();
        assert_eq!(
            host_key_algorithms("nas.lan", 22, file).as_deref(),
            Some("ssh-ed25519")
        );
        assert_eq!(host_key_algorithms("nas.lan", 2222, file), None);
        assert_eq!(host_key_algorithms("other.lan", 22, file), None);
        assert_eq!(host_key_algorithms("nas.lan", 22, "/no/such/file"), None);
    }

    #[test]
    fn split_and_join() {
        assert_eq!(split_root("/a/b").unwrap(), ("/a", "b"));
        assert_eq!(split_root("/a").unwrap(), ("/", "a"));
        assert_eq!(split_root("rel").unwrap(), ("", "rel"));
        assert!(split_root("/").is_err());
        assert!(split_root("").is_err());
        assert_eq!(join("/", "x"), "/x");
        assert_eq!(join("", "x"), "x");
        assert_eq!(join("/a/", ""), "/a/");
    }
}
