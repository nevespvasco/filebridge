//! Builds rclone fs strings. Backends are always on-the-fly connection strings
//! (`:sftp,host=…:path`), so no `rclone.conf` is ever written.

use std::path::Path;

use filebridge_transport_api::{Backend, Destination, Result, TransportError};

/// fs string for `sub` (relative, may be empty) under the destination root.
pub fn dest_fs(dest: &Destination, sub: &str) -> Result<String> {
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
            if password.is_some() {
                return Err(TransportError::fatal(
                    "SFTP password auth needs the keyring (Phase 1); use a key or the SSH agent",
                ));
            }
            let mut params = vec![("host", host.clone())];
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
            params.push(("known_hosts_file", known_hosts));
            let params: Vec<String> = params
                .into_iter()
                .map(|(k, v)| format!("{k}={}", quote(&v)))
                .collect();
            Ok(format!(":sftp,{}:{path}", params.join(",")))
        }
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
            dest_fs(&dest, "Backup").unwrap(),
            ":sftp,host=\"nas.lan\",port=\"2222\",user=\"ana\",key_file=\"/k/id,ed25519\",\
             known_hosts_file=\"/k/known_hosts\":/srv/files/Backup"
        );
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
