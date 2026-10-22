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
    /// The Nextcloud user whose files these are.
    pub user: String,
}

/// Whether `host` is this machine: the only place plain `http` is accepted (tests, a tunnel).
fn loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// The server's base address from what a user types: `cloud.example.com`,
/// `https://cloud.example.com/nextcloud/`, or a WebDAV address copied from Nextcloud's settings
/// (`…/remote.php/dav/files/alice/`, `…/remote.php/webdav/`). HTTPS only, except on this machine.
pub fn server_url(input: &str) -> std::result::Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("enter the Nextcloud address".into());
    }
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let (scheme, rest) = with_scheme
        .split_once("://")
        .ok_or_else(|| format!("{input:?} is not a web address"))?;
    let base = match rest.find("/remote.php") {
        Some(i) => &rest[..i],
        None => rest,
    };
    let base = base.trim_end_matches('/');
    let host = base.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let hostname = match host.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    };
    if hostname.is_empty() || base.contains(['?', '#', ' ']) {
        return Err(format!("{input:?} is not a web address"));
    }
    match scheme.to_ascii_lowercase().as_str() {
        "https" => {}
        "http" if loopback(hostname) => {}
        "http" => {
            return Err(
                "the address must start with https://: an app password must not travel unencrypted"
                    .into(),
            );
        }
        other => return Err(format!("{other}:// is not a web address")),
    }
    Ok(format!("{}://{base}", scheme.to_ascii_lowercase()))
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

/// The WebDAV address of `user`'s files on `server` (a [`server_url`]).
pub fn dav_url(server: &str, user: &str) -> String {
    format!("{server}/remote.php/dav/files/{}/", segment(user))
}

impl NextcloudWebdavProfile {
    pub fn validate(&self) -> Result<()> {
        if self.user.trim().is_empty() {
            return Err(IntegrationError::fatal("the Nextcloud user is empty"));
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
        if *url != dav_url(&self.server, &self.user) {
            return Err(IntegrationError::fatal(format!(
                "the WebDAV address {url:?} is not {:?}'s files on {}",
                self.user, self.server
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
