//! WebDAV mode (docs/05): files go through Nextcloud's own API (rclone's `webdav` backend with
//! vendor `nextcloud`), signed in with an app password kept in the OS keyring. Nextcloud indexes
//! what it receives, so there is nothing to do after the transfer, and nothing to reach on the
//! server but its HTTPS address.

use filebridge_integration_api::{AfterTransferReport, IntegrationError, JobContext, Result};
use filebridge_transport_api::{
    Backend, Check, CheckStatus, Destination, PreflightReport, RemotePath,
};
use serde::{Deserialize, Serialize};

/// rclone's name for Nextcloud's WebDAV flavour (chunked uploads, checksums).
pub const VENDOR: &str = "nextcloud";

/// What a WebDAV-mode profile stores besides its destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextcloudWebdavProfile {
    /// The server's address as normalised by [`server_url`], e.g. `https://cloud.example.com`.
    pub server: String,
    /// The login the app password belongs to.
    pub user: String,
    /// The user's id in WebDAV paths (`remote.php/dav/files/<id>/`) when it isn't the login, as
    /// with email or LDAP logins; taken from a WebDAV address the user pasted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// What a typed address says: the server's base address, and the user id when it was a WebDAV
/// address copied from Nextcloud (`…/remote.php/dav/files/<id>/`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddress {
    pub server: String,
    pub user_id: Option<String>,
}

/// Whether `host` is this machine: the only place plain `http` is accepted (tests, a tunnel).
/// `localhost` or a loopback IP as parsed, never a name that merely starts like one
/// (`127.example.org`).
fn loopback(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// `%XX` escapes of one path segment, decoded; `None` if they aren't UTF-8.
fn percent_decoded(segment: &str) -> Option<String> {
    let b = segment.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%')
            .then(|| segment.get(i + 1..i + 3))
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Parses what a user types: `cloud.example.com`, `https://cloud.example.com/nextcloud/`, or a
/// WebDAV address copied from Nextcloud's settings (`…/remote.php/dav/files/alice/`,
/// `…/remote.php/webdav/`). HTTPS only, except on this machine; no user or password in the
/// address (it would be stored and shown).
pub fn parse_address(input: &str) -> std::result::Result<ServerAddress, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("enter the Nextcloud address".into());
    }
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let not_an_address = || format!("{input:?} is not a web address");
    let mut url = url::Url::parse(&with_scheme).map_err(|_| not_an_address())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(
            "leave the user and password out of the address: enter them in their own fields".into(),
        );
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(not_an_address());
    }
    let host = url.host().ok_or_else(not_an_address)?;
    match url.scheme() {
        "https" => {}
        "http" if loopback(&host) => {}
        "http" => {
            return Err(
                "the address must start with https://: an app password must not travel unencrypted"
                    .into(),
            );
        }
        other => return Err(format!("{other}:// is not a web address")),
    }
    let segments: Vec<&str> = url.path_segments().map_or(Vec::new(), Iterator::collect);
    let dav = segments.iter().position(|s| *s == "remote.php");
    let user_id = dav.and_then(|i| match &segments[i + 1..] {
        ["dav", "files", id, ..] if !id.is_empty() => percent_decoded(id),
        _ => None,
    });
    let base: Vec<&str> = segments[..dav.unwrap_or(segments.len())]
        .iter()
        .copied()
        .filter(|s| !s.is_empty())
        .collect();
    url.set_path(&base.join("/"));
    let server = url.as_str().trim_end_matches('/').to_owned();
    Ok(ServerAddress { server, user_id })
}

/// The server's base address from what a user types (see [`parse_address`]).
pub fn server_url(input: &str) -> std::result::Result<String, String> {
    parse_address(input).map(|a| a.server)
}

/// `user` as one URL path segment.
fn segment(user: &str) -> String {
    user.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The WebDAV address of the files of the user with id `user_id` on `server` (a
/// [`server_url`]).
pub fn dav_url(server: &str, user_id: &str) -> String {
    format!("{server}/remote.php/dav/files/{}/", segment(user_id))
}

impl NextcloudWebdavProfile {
    /// The id in the user's WebDAV path: [`Self::user_id`], or the login.
    pub fn files_id(&self) -> &str {
        self.user_id.as_deref().unwrap_or(&self.user)
    }

    pub fn validate(&self) -> Result<()> {
        if self.user.trim().is_empty() {
            return Err(IntegrationError::fatal("the Nextcloud user is empty"));
        }
        if self
            .user_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
        {
            return Err(IntegrationError::fatal("the Nextcloud user id is empty"));
        }
        let normalised = server_url(&self.server).map_err(IntegrationError::fatal)?;
        if normalised != self.server {
            return Err(IntegrationError::fatal(format!(
                "the server address {:?} is not normalised (expected {normalised:?})",
                self.server
            )));
        }
        Ok(())
    }

    /// A WebDAV-mode profile sends with the `nextcloud` WebDAV flavour to this user's files.
    pub fn check(&self, destination: &Destination) -> Result<()> {
        self.validate()?;
        let Backend::Webdav {
            url,
            user,
            password,
            vendor,
        } = &destination.backend
        else {
            return Err(IntegrationError::fatal(
                "Nextcloud's WebDAV mode sends over WebDAV",
            ));
        };
        if *url != dav_url(&self.server, self.files_id()) {
            return Err(IntegrationError::fatal(format!(
                "the WebDAV address {url:?} is not {:?}'s files on {}",
                self.files_id(),
                self.server
            )));
        }
        if *user != self.user {
            return Err(IntegrationError::fatal(
                "the WebDAV user and the Nextcloud user differ",
            ));
        }
        if vendor.as_deref() != Some(VENDOR) {
            return Err(IntegrationError::fatal(
                "the WebDAV flavour must be nextcloud",
            ));
        }
        if password.is_none() {
            return Err(IntegrationError::fatal(
                "Nextcloud's WebDAV mode needs an app password",
            ));
        }
        if destination.root.starts_with('/') {
            return Err(IntegrationError::fatal(
                "the folder is relative to the user's files, without a leading /",
            ));
        }
        Ok(())
    }
}

/// Nothing to check beyond the transport's own preflight (sign-in, quota, the folder): the
/// whole connection is HTTPS to Nextcloud.
pub fn preflight(profile: &NextcloudWebdavProfile, _ctx: &JobContext) -> Result<PreflightReport> {
    profile.validate()?;
    Ok(PreflightReport {
        writable: true,
        checks: vec![Check {
            id: "nextcloud-webdav".into(),
            status: CheckStatus::Ok,
            message: format!(
                "WebDAV mode: Nextcloud on {} indexes files as they arrive",
                profile.server
            ),
        }],
        ..PreflightReport::default()
    })
}

/// Nextcloud indexed every file as it arrived through its API.
pub fn after_transfer(changed: &[RemotePath]) -> AfterTransferReport {
    AfterTransferReport {
        updated: Vec::new(),
        items: changed.len() as u64,
        problems: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use filebridge_transport_api::KeyringRef;

    #[test]
    fn what_people_type_becomes_one_address() {
        for input in [
            "cloud.example.com",
            "https://cloud.example.com",
            "https://cloud.example.com/",
            "HTTPS://cloud.example.com/remote.php/dav/files/alice/",
            "https://cloud.example.com/remote.php/webdav/",
        ] {
            assert_eq!(
                server_url(input).as_deref(),
                Ok("https://cloud.example.com"),
                "{input}"
            );
        }
        assert_eq!(
            server_url("https://example.org:8443/nextcloud/remote.php/dav/files/bob/").as_deref(),
            Ok("https://example.org:8443/nextcloud")
        );
        assert_eq!(
            server_url("http://127.0.0.1:8080").as_deref(),
            Ok("http://127.0.0.1:8080")
        );
        for local in [
            "http://localhost:8080",
            "http://LOCALHOST",
            "http://[::1]:8080",
        ] {
            assert!(server_url(local).is_ok(), "{local}");
        }
        // Normalised once, the same again: a stored address passes `validate`.
        for input in [
            "Cloud.Example.com:443/a//b/",
            "https://example.org:8443/nc/",
        ] {
            let once = server_url(input).unwrap();
            assert_eq!(server_url(&once).unwrap(), once, "{input}");
        }
        assert_eq!(
            server_url("cloud.example.com:443").as_deref(),
            Ok("https://cloud.example.com")
        );
    }

    #[test]
    fn names_that_only_look_local_need_https() {
        for input in [
            "http://127.evil.example",
            "http://127.0.0.1.nip.io",
            "http://localhost.evil.example",
            "http://[::2]",
        ] {
            let err = server_url(input).unwrap_err();
            assert!(err.contains("https://"), "{input}: {err}");
        }
    }

    #[test]
    fn credentials_and_malformed_addresses_are_refused() {
        for input in [
            "https://alice:apppw@cloud.example.com",
            "https://alice@cloud.example.com/remote.php/dav/files/alice/",
            "http://alice:apppw@127.0.0.1:8080",
        ] {
            let err = server_url(input).unwrap_err();
            assert!(err.contains("leave the user"), "{input}: {err}");
            assert!(!err.contains("apppw"), "{err}");
        }
        for input in [
            "https://cloud.example.com:99999",
            "https://cloud.example.com:8a",
            "https://[::1",
            "https://[nothex]/",
            "https://cloud.example.com/?x=1",
            "https://cloud.example.com/#top",
        ] {
            assert!(server_url(input).is_err(), "{input}");
        }
    }

    #[test]
    fn a_pasted_webdav_address_gives_the_user_id() {
        let a =
            parse_address("https://cloud.example.com/remote.php/dav/files/5b0f3c1e-ldap-id/Fotos")
                .unwrap();
        assert_eq!(a.server, "https://cloud.example.com");
        assert_eq!(a.user_id.as_deref(), Some("5b0f3c1e-ldap-id"));
        let a =
            parse_address("cloud.example.com/remote.php/dav/files/maria%40example.com/").unwrap();
        assert_eq!(a.user_id.as_deref(), Some("maria@example.com"));
        assert_eq!(
            parse_address("https://cloud.example.com/remote.php/webdav/")
                .unwrap()
                .user_id,
            None
        );
        assert_eq!(parse_address("cloud.example.com").unwrap().user_id, None);
    }

    #[test]
    fn plain_http_is_refused_off_this_machine() {
        let err = server_url("http://cloud.example.com").unwrap_err();
        assert!(err.contains("https://"), "{err}");
        assert!(server_url("").is_err());
        assert!(server_url("ftp://cloud.example.com").is_err());
        assert!(server_url("https://").is_err());
    }

    #[test]
    fn the_user_is_one_path_segment() {
        assert_eq!(
            dav_url("https://cloud.example.com", "maria.silva@example.com"),
            "https://cloud.example.com/remote.php/dav/files/maria.silva%40example.com/"
        );
        assert_eq!(
            dav_url("https://cloud.example.com", "Ana Sá"),
            "https://cloud.example.com/remote.php/dav/files/Ana%20S%C3%A1/"
        );
    }

    fn profile() -> (NextcloudWebdavProfile, Destination) {
        let p = NextcloudWebdavProfile {
            server: "https://cloud.example.com".into(),
            user: "alice".into(),
            user_id: None,
        };
        let d = Destination {
            transport: "rclone".into(),
            backend: Backend::Webdav {
                url: dav_url(&p.server, &p.user),
                user: "alice".into(),
                password: Some(KeyringRef("filebridge/p/app-password".into())),
                vendor: Some(VENDOR.into()),
            },
            root: "Backup".into(),
        };
        (p, d)
    }

    #[test]
    fn a_consistent_profile_passes() {
        let (p, d) = profile();
        p.check(&d).unwrap();
    }

    #[test]
    fn an_email_login_sends_to_its_user_id() {
        let (mut p, mut d) = profile();
        p.user = "alice@example.com".into();
        p.user_id = Some("alice".into());
        if let Backend::Webdav { user, .. } = &mut d.backend {
            *user = "alice@example.com".into();
        }
        p.check(&d).unwrap();
        p.user_id = None;
        assert!(p.check(&d).is_err(), "the login's own path is another one");
    }

    #[test]
    fn inconsistent_profiles_are_refused() {
        let (p, d) = profile();
        let mut other_user = d.clone();
        if let Backend::Webdav { user, .. } = &mut other_user.backend {
            *user = "bob".into();
        }
        assert!(p.check(&other_user).is_err());
        let mut no_password = d.clone();
        if let Backend::Webdav { password, .. } = &mut no_password.backend {
            *password = None;
        }
        assert!(p.check(&no_password).is_err());
        let absolute = Destination {
            root: "/Backup".into(),
            ..d.clone()
        };
        assert!(p.check(&absolute).is_err());
        let sftp = Destination {
            backend: Backend::Local,
            ..d
        };
        assert!(p.check(&sftp).is_err());
    }
}
