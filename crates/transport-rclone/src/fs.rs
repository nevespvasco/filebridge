//! Builds rclone fs strings. Backends are always on-the-fly connection strings
//! (`:sftp,host=…:path`), so no `rclone.conf` is ever written; secrets go in them only in the
//! form rclone takes (an obscured password), over the local authenticated rc API, and are
//! scrubbed from rclone's error messages and log lines ([`Scrubber`], [`redact`]).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use filebridge_transport_api::{Backend, Destination, Result, SecretValue, TransportError};

/// A destination's secrets, fetched from the keyring for one call, as rclone takes them. While
/// it lives, the [`Scrubber`] that made it removes these values from every message.
#[derive(Debug, Default)]
pub struct Creds {
    /// The password as rclone takes it: obscured (`core/obscure`) for its `pass=` parameter
    /// (SFTP, WebDAV, SMB), as given for S3's `secret_access_key=`.
    pub pass: Option<SecretValue>,
    /// filebridge's own SSH key (ADR 0012), PEM on one line with `\n` escapes: rclone's
    /// `key_pem` parameter. Never written to a file.
    pub key_pem: Option<SecretValue>,
    pub(crate) held: Vec<ScrubGuard>,
}

/// Values shorter than this are scrubbed only where they follow their parameter's name
/// ([`redact`]): replacing every `ab` in a message would make it unreadable and hide nothing.
const MIN_SCRUBBED: usize = 4;

/// The secret values in use right now. [`Scrubber::scrub`] replaces each of them, in every form
/// rclone may print it (as given, quoted for a connection string, Go's `%q`, a PEM's lines), on
/// top of [`redact`]'s parameter parsing: a value can reach a message without its parameter
/// name. Shared by an rcd's client (its errors) and its stderr reader (its log lines).
#[derive(Clone, Default)]
pub struct Scrubber(Arc<Mutex<HashMap<String, usize>>>);

impl Scrubber {
    /// Scrubs `secret` from messages until the guard is dropped.
    pub(crate) fn hold(&self, secret: &str) -> ScrubGuard {
        let forms = forms(secret);
        let mut map = self.0.lock().expect("scrubber lock");
        for f in &forms {
            *map.entry(f.clone()).or_default() += 1;
        }
        ScrubGuard {
            scrubber: self.clone(),
            forms,
        }
    }

    /// `message` without any secret parameter's value nor any value held now.
    pub fn scrub(&self, message: &str) -> String {
        let mut out = redact(message);
        let map = self.0.lock().expect("scrubber lock");
        let mut values: Vec<&String> = map.keys().collect();
        // Longest first, so a value that contains another goes whole.
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        for v in values {
            if out.contains(v.as_str()) {
                out = out.replace(v.as_str(), "***");
            }
        }
        out
    }
}

/// Keeps a secret's forms in its [`Scrubber`]; they go when the last holder drops.
pub(crate) struct ScrubGuard {
    scrubber: Scrubber,
    forms: Vec<String>,
}

impl std::fmt::Debug for ScrubGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScrubGuard")
    }
}

impl Drop for ScrubGuard {
    fn drop(&mut self) {
        let mut map = self.scrubber.0.lock().expect("scrubber lock");
        for f in &self.forms {
            if let Some(n) = map.get_mut(f) {
                *n -= 1;
                if *n == 0 {
                    map.remove(f);
                }
            }
        }
    }
}

/// Go's `%q` of `s`, without the outer quotes: how rclone dumps a string in some errors.
fn go_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// Every form of `secret` a message may carry.
fn forms(secret: &str) -> Vec<String> {
    let doubled = secret.replace('"', "\"\"");
    let mut out = vec![
        secret.to_owned(),
        go_quoted(secret),
        go_quoted(&doubled),
        doubled,
    ];
    // A key: its lines on their own too (the PEM's body), with real or escaped line breaks.
    if secret.contains('\n') || secret.contains("\\n") {
        out.extend(
            secret
                .split('\n')
                .flat_map(|l| l.split("\\n"))
                .map(str::trim)
                .filter(|l| l.len() >= 16 && !l.starts_with("-----"))
                .map(str::to_owned),
        );
    }
    out.retain(|f| f.len() >= MIN_SCRUBBED);
    out.sort();
    out.dedup();
    out
}

/// The connection-string parameters that carry secrets; their values never leave this crate.
const SECRET_PARAMS: [&str; 3] = ["pass", "key_pem", "secret_access_key"];

/// fs string for the folder `sub` (relative, may be empty) under the destination root.
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
            key,
        } => {
            let mut params = vec![("host", host.clone())];
            if password.is_some() {
                params.push(("pass", needed(&creds.pass, "SFTP")?));
            }
            if key.is_some() {
                let pem = creds.key_pem.as_ref().ok_or_else(|| {
                    TransportError::new(
                        filebridge_transport_api::ErrorKind::Auth,
                        "filebridge's SSH key is not in the keyring; use its key again or Remove access",
                    )
                })?;
                params.push(("key_pem", pem.expose().to_owned()));
            }
            if let Some(port) = port {
                params.push(("port", port.to_string()));
            }
            if let Some(user) = user {
                params.push(("user", user.clone()));
            }
            if let (None, Some(file)) = (key, key_file) {
                params.push(("key_file", utf8(file)?.to_owned()));
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
            // A symbolic link at the destination is listed as what it points to otherwise, so
            // "Repair destination" would rename (and Nextcloud scan) things outside the folder.
            params.push(("skip_links", "true".into()));
            Ok(format!(":sftp,{}:{path}", connection(params)))
        }
        Backend::Webdav {
            url,
            user,
            password,
            vendor,
            skip_certificate_check,
        } => {
            let mut params = vec![("url", url.clone()), ("user", user.clone())];
            if let Some(vendor) = vendor {
                params.push(("vendor", vendor.clone()));
            }
            if password.is_some() {
                params.push(("pass", needed(&creds.pass, "WebDAV")?));
            }
            // rclone's global option, for this remote only (ADR 0035).
            if *skip_certificate_check {
                params.push(("override.no_check_certificate", "true".into()));
            }
            Ok(format!(":webdav,{}:{path}", connection(params)))
        }
        Backend::Smb {
            host,
            port,
            user,
            domain,
            share,
            password,
        } => {
            let mut params = vec![("host", host.clone())];
            if let Some(port) = port {
                params.push(("port", port.to_string()));
            }
            // rclone signs in as the local user when none is given; an SMB profile without one
            // is the guest.
            params.push(("user", user.clone().unwrap_or_else(|| "guest".into())));
            if let Some(domain) = domain {
                params.push(("domain", domain.clone()));
            }
            if password.is_some() {
                params.push(("pass", needed(&creds.pass, "SMB")?));
            }
            let path = join(share.trim_matches('/'), &inside(&dest.root, sub));
            Ok(format!(":smb,{}:{path}", connection(params)))
        }
        Backend::S3 {
            provider,
            endpoint,
            region,
            bucket,
            access_key_id,
            secret_access_key,
            path_style,
        } => {
            let mut params = vec![(
                "provider",
                provider.clone().unwrap_or_else(|| "Other".into()),
            )];
            if let Some(endpoint) = endpoint {
                params.push(("endpoint", endpoint.clone()));
            }
            if let Some(region) = region {
                params.push(("region", region.clone()));
            }
            if let Some(id) = access_key_id {
                params.push(("access_key_id", id.clone()));
            }
            if secret_access_key.is_some() {
                let secret = creds.pass.as_ref().ok_or_else(|| {
                    TransportError::new(
                        filebridge_transport_api::ErrorKind::Auth,
                        "the S3 secret access key is not in the keyring; set it again",
                    )
                })?;
                params.push(("secret_access_key", secret.expose().to_owned()));
            }
            params.push(("force_path_style", path_style.to_string()));
            // A folder inside the bucket ends in `/`: otherwise rclone first asks whether it is
            // a file, and when the key is refused it takes the refusal for "a file", so a wrong
            // secret would read "is a file not a directory" instead of the server's answer.
            let folder = inside(&dest.root, sub);
            let bucket = bucket.trim_matches('/');
            let path = match folder.as_str() {
                "" => bucket.to_owned(),
                f => format!("{bucket}/{f}/"),
            };
            Ok(format!(":s3,{}:{path}", connection(params)))
        }
    }
}

/// `sub` under `root` for backends whose root is a folder inside a share or a bucket: both
/// relative, without the slashes at their ends.
fn inside(root: &str, sub: &str) -> String {
    join(root.trim_matches('/'), sub)
}

fn connection(params: Vec<(&str, String)>) -> String {
    params
        .into_iter()
        .map(|(k, v)| format!("{k}={}", quote(&v)))
        .collect::<Vec<_>>()
        .join(",")
}

/// The obscured password a profile says it has: missing from the keyring is an `Auth` error.
fn needed(pass: &Option<SecretValue>, what: &str) -> Result<String> {
    pass.as_ref().map(|p| p.expose().to_owned()).ok_or_else(|| {
        TransportError::new(
            filebridge_transport_api::ErrorKind::Auth,
            format!("the {what} password is not in the keyring; set it again"),
        )
    })
}

/// `message` with the value of every secret connection-string parameter (`pass="…"`,
/// `key_pem=…`, and Go's `%q` of them, `pass=\"…\"`) replaced by `***`: rclone repeats the fs
/// string in some errors.
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
        let end = if value.starts_with("\\\"") {
            quoted_end(value, "\\\"")
        } else if value.starts_with('"') {
            quoted_end(value, "\"")
        } else {
            value
                .find([',', ':', ' ', '"', '\\'])
                .unwrap_or(value.len())
        };
        rest = &value[end..];
    }
}

/// Where a value that starts with the quote `q` ends: after the closing `q` that isn't doubled
/// (a quote inside is written twice), or at the end of the message.
fn quoted_end(value: &str, q: &str) -> usize {
    let mut j = q.len();
    while let Some(k) = value[j..].find(q) {
        let after = j + k + q.len();
        if value[after..].starts_with(q) {
            j = after + q.len();
        } else {
            return after;
        }
    }
    value.len()
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
                key: None,
            },
            root: "/srv/files/".into(),
        };
        assert_eq!(
            dest_fs(&dest, &Creds::default(), "Backup").unwrap(),
            ":sftp,host=\"nas.lan\",port=\"2222\",user=\"ana\",key_file=\"/k/id,ed25519\",\
             known_hosts_file=\"/k/known_hosts\",skip_links=\"true\":/srv/files/Backup"
        );
    }

    #[test]
    fn smb_connection_string() {
        let dest = |user: Option<&str>, password: bool, root: &str| Destination {
            transport: "rclone".into(),
            backend: Backend::Smb {
                host: "nas.lan".into(),
                port: Some(4450),
                user: user.map(Into::into),
                domain: Some("CASA".into()),
                share: "/backup/".into(),
                password: password.then(|| {
                    filebridge_transport_api::KeyringRef("filebridge/p/app-password".into())
                }),
            },
            root: root.into(),
        };
        let creds = Creds {
            pass: Some(SecretValue::new("obscured".into())),
            ..Creds::default()
        };
        assert_eq!(
            dest_fs(&dest(Some("ana"), true, "/Fotos/"), &creds, "2026").unwrap(),
            ":smb,host=\"nas.lan\",port=\"4450\",user=\"ana\",domain=\"CASA\",\
             pass=\"obscured\":backup/Fotos/2026"
        );
        assert_eq!(
            dest_fs(&dest(None, false, ""), &Creds::default(), "").unwrap(),
            ":smb,host=\"nas.lan\",port=\"4450\",user=\"guest\",domain=\"CASA\":backup"
        );
        let err = dest_fs(&dest(Some("ana"), true, ""), &Creds::default(), "").unwrap_err();
        assert_eq!(err.kind, filebridge_transport_api::ErrorKind::Auth);
    }

    #[test]
    fn webdav_connection_string() {
        let dest = |skip_certificate_check: bool| Destination {
            transport: "rclone".into(),
            backend: Backend::Webdav {
                url: "https://pve.example.lan/remote.php/dav/files/ana/".into(),
                user: "ana".into(),
                password: Some(filebridge_transport_api::KeyringRef(
                    "filebridge/p/app-password".into(),
                )),
                vendor: Some("nextcloud".into()),
                skip_certificate_check,
            },
            root: "Fotos".into(),
        };
        let creds = Creds {
            pass: Some(SecretValue::new("obscured".into())),
            ..Creds::default()
        };
        assert_eq!(
            dest_fs(&dest(false), &creds, "2026").unwrap(),
            ":webdav,url=\"https://pve.example.lan/remote.php/dav/files/ana/\",user=\"ana\",\
             vendor=\"nextcloud\",pass=\"obscured\":Fotos/2026"
        );
        assert_eq!(
            dest_fs(&dest(true), &creds, "").unwrap(),
            ":webdav,url=\"https://pve.example.lan/remote.php/dav/files/ana/\",user=\"ana\",\
             vendor=\"nextcloud\",pass=\"obscured\",override.no_check_certificate=\"true\":Fotos"
        );
    }

    #[test]
    fn s3_connection_string() {
        let dest = |root: &str| Destination {
            transport: "rclone".into(),
            backend: Backend::S3 {
                provider: Some("Minio".into()),
                endpoint: Some("http://minio.lan:9000".into()),
                region: Some("us-east-1".into()),
                bucket: "fotos".into(),
                access_key_id: Some("AKIDEXAMPLE".into()),
                secret_access_key: Some(filebridge_transport_api::KeyringRef(
                    "filebridge/p/app-password".into(),
                )),
                path_style: true,
            },
            root: root.into(),
        };
        let creds = Creds {
            pass: Some(SecretValue::new("s3cret/Key+1".into())),
            ..Creds::default()
        };
        let params = "provider=\"Minio\",endpoint=\"http://minio.lan:9000\",\
                      region=\"us-east-1\",access_key_id=\"AKIDEXAMPLE\",\
                      secret_access_key=\"s3cret/Key+1\",force_path_style=\"true\"";
        // Folders inside the bucket end in `/`; the bucket itself doesn't.
        assert_eq!(
            dest_fs(&dest("/backup/"), &creds, "2026/x").unwrap(),
            format!(":s3,{params}:fotos/backup/2026/x/")
        );
        assert_eq!(
            dest_fs(&dest(""), &creds, "").unwrap(),
            format!(":s3,{params}:fotos")
        );
        let err = dest_fs(&dest(""), &Creds::default(), "").unwrap_err();
        assert_eq!(err.kind, filebridge_transport_api::ErrorKind::Auth);
        // The secret is plain in the connection string, so it is scrubbed by name too.
        assert_eq!(
            redact(&dest_fs(&dest(""), &creds, "").unwrap()),
            format!(":s3,{}:fotos", params.replace("\"s3cret/Key+1\"", "***"))
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
        // Go's %q of a connection string: quotes escaped, doubled ones too.
        let q = r#"fs "\":sftp,host=\"nas\",pass=\"Ab\"\"c\",key_pem=\"-----BEGIN\\nxyz\":/\"""#;
        let out = redact(q);
        assert!(!out.contains("Ab") && !out.contains("xyz"), "{out}");
        assert!(out.contains(r#"host=\"nas\""#), "{out}");
    }

    #[test]
    fn held_values_are_scrubbed_in_every_form_while_held() {
        let scrubber = Scrubber::default();
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ\n-----END OPENSSH PRIVATE KEY-----\n";
        let escaped = pem.trim_end().replace('\n', "\\n");
        let guards = [
            scrubber.hold("obscured-pass\"1"),
            scrubber.hold(pem),
            scrubber.hold(&escaped),
            scrubber.hold("ab"),
        ];
        for msg in [
            "failed: obscured-pass\"1".to_owned(),
            format!("bad key {escaped:?}"),
            format!("bad key {pem}"),
            "line b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ seen".to_owned(),
            r#"fs ":sftp,pass=\"obscured-pass\"\"1\"""#.to_owned(),
            "dump obscured-pass\\\"1".to_owned(),
        ] {
            let out = scrubber.scrub(&msg);
            assert!(
                !out.contains("obscured-pass") && !out.contains("b3BlbnNzaC1rZXkt"),
                "{msg} -> {out}"
            );
        }
        assert_eq!(
            scrubber.scrub("abc ab"),
            "abc ab",
            "too short to scrub alone"
        );
        drop(guards);
        assert_eq!(scrubber.scrub("obscured-pass\"1"), "obscured-pass\"1");
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
                key: None,
            },
            root: "/srv".into(),
        };
        let err = dest_fs(&dest, &Creds::default(), "").unwrap_err();
        assert_eq!(err.kind, filebridge_transport_api::ErrorKind::Auth);
        let creds = Creds {
            pass: Some(SecretValue::new("obscured".into())),
            ..Creds::default()
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
    fn join_paths() {
        assert_eq!(join("/", "x"), "/x");
        assert_eq!(join("", "x"), "x");
        assert_eq!(join("/a/", ""), "/a/");
    }
}
