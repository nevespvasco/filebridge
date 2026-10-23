//! What an Immich profile stores besides its destination, and the checks every saved one passes.

use filebridge_integration_api::{IntegrationError, Result};
use filebridge_transport_api::{Backend, Destination, KeyringRef};
use serde::{Deserialize, Serialize};

/// An Immich profile's settings. The destination (SFTP, or a local folder) writes into a folder
/// of an external library; these say which Immich and which library, and where that folder is
/// as Immich sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImmichSettings {
    /// The server's address as normalised by [`server_url`], e.g. `https://photos.example.com`.
    pub server: String,
    /// The API key, in the OS keyring (ADR 0004). Its user must be an Immich administrator.
    pub api_key: KeyringRef,
    /// The external library's id (a UUID).
    pub library_id: String,
    /// The destination's root folder as Immich sees it: inside one of the library's import
    /// paths. Differs from the SFTP path when Immich runs in a container with the folder
    /// mounted elsewhere (`/srv/photos` on the host, `/mnt/photos` in the container).
    pub import_path: String,
}

impl ImmichSettings {
    pub fn validate(&self) -> Result<()> {
        let normalised = server_url(&self.server).map_err(IntegrationError::fatal)?;
        if normalised != self.server {
            return Err(IntegrationError::fatal(format!(
                "the Immich address {:?} is not normalised (expected {normalised:?})",
                self.server
            )));
        }
        if self.api_key.0.trim().is_empty() {
            return Err(IntegrationError::fatal("the Immich API key is missing"));
        }
        if !valid_library_id(&self.library_id) {
            return Err(IntegrationError::fatal(format!(
                "{:?} is not an Immich library id",
                self.library_id
            )));
        }
        let clean = clean_path(&self.import_path).map_err(IntegrationError::fatal)?;
        if clean != self.import_path {
            return Err(IntegrationError::fatal(format!(
                "the import path {:?} is not normalised (expected {clean:?})",
                self.import_path
            )));
        }
        Ok(())
    }

    /// The destination writes a folder Immich can read: an SFTP server (an absolute folder) or
    /// a folder on this machine.
    pub fn check(&self, destination: &Destination) -> Result<()> {
        self.validate()?;
        match &destination.backend {
            Backend::Sftp { .. } if !destination.root.starts_with('/') => {
                Err(IntegrationError::fatal(
                    "the folder on the SFTP server must be absolute (start with /)",
                ))
            }
            Backend::Sftp { .. } | Backend::Local if destination.root.trim().is_empty() => Err(
                IntegrationError::fatal("the destination folder of an Immich profile is empty"),
            ),
            Backend::Sftp { .. } | Backend::Local => Ok(()),
            other => Err(IntegrationError::fatal(format!(
                "Immich reads an external library from a folder: send over SFTP or to a local \
                 folder, not {}",
                other.kind()
            ))),
        }
    }
}

/// Whether `host` is this machine: the only place plain `http` is accepted. `localhost` or a
/// loopback IP as parsed, never a name that merely starts like one (`127.example.org`).
fn loopback(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// The server's base address from what a user types: `photos.example.com`,
/// `https://photos.example.com:2283/`, or its API address (`…/api`). HTTPS only, except on this
/// machine (the API key travels with every request); no user or password in the address.
pub fn server_url(input: &str) -> std::result::Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("enter the Immich address".into());
    }
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let not_an_address = || format!("{input:?} is not a web address");
    let mut url = url::Url::parse(&with_scheme).map_err(|_| not_an_address())?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("leave the user and password out of the address".into());
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
                "the address must start with https://: the API key must not travel unencrypted"
                    .into(),
            );
        }
        other => return Err(format!("{other}:// is not a web address")),
    }
    let mut segments: Vec<&str> = url
        .path_segments()
        .map_or(Vec::new(), Iterator::collect::<Vec<_>>)
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    if segments.last() == Some(&"api") {
        segments.pop();
    }
    url.set_path(&segments.join("/"));
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// A library id as Immich makes them: a UUID. Checked before it goes into a URL path.
pub fn valid_library_id(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// An absolute POSIX path without a trailing `/`, `.`, `..` or empty parts: how import paths
/// are compared.
pub fn clean_path(path: &str) -> std::result::Result<String, String> {
    let path = path.trim();
    if !path.starts_with('/') {
        return Err(format!(
            "{path:?} is not an absolute path (as Immich sees it, e.g. /mnt/photos)"
        ));
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    if parts.iter().any(|p| *p == "." || *p == "..") {
        return Err(format!("{path:?} has . or .. in it"));
    }
    Ok(format!("/{}", parts.join("/")))
}

/// Whether the import path `library_path` holds `path` (both as Immich sees them).
pub fn covers(library_path: &str, path: &str) -> bool {
    let (Ok(lib), Ok(path)) = (clean_path(library_path), clean_path(path)) else {
        return false;
    };
    lib == "/" || path == lib || path.starts_with(&format!("{lib}/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIB: &str = "6f1c2a4e-0b7d-4c55-9a8e-3d2f1b0c9e77";

    fn settings() -> ImmichSettings {
        ImmichSettings {
            server: "https://photos.example.com".into(),
            api_key: KeyringRef("filebridge/p/api-key".into()),
            library_id: LIB.into(),
            import_path: "/mnt/photos/Holidays".into(),
        }
    }

    fn sftp(root: &str) -> Destination {
        Destination {
            transport: "rclone".into(),
            backend: Backend::Sftp {
                host: "nas.example.com".into(),
                port: None,
                user: Some("photos".into()),
                key_file: None,
                known_hosts_file: None,
                password: None,
                key: None,
            },
            root: root.into(),
        }
    }

    #[test]
    fn what_people_type_becomes_one_address() {
        for input in [
            "photos.example.com",
            "https://photos.example.com",
            "HTTPS://Photos.Example.com/",
            "https://photos.example.com/api",
            "https://photos.example.com/api/",
            "photos.example.com:443",
        ] {
            assert_eq!(
                server_url(input).as_deref(),
                Ok("https://photos.example.com"),
                "{input}"
            );
        }
        assert_eq!(
            server_url("https://example.org:8443/immich/api").as_deref(),
            Ok("https://example.org:8443/immich")
        );
        assert_eq!(
            server_url("http://127.0.0.1:2283").as_deref(),
            Ok("http://127.0.0.1:2283")
        );
        assert!(server_url("http://localhost:2283").is_ok());
        assert!(server_url("http://[::1]:2283").is_ok());
        let once = server_url("Photos.Example.com:8443//a/").unwrap();
        assert_eq!(server_url(&once).unwrap(), once);
    }

    #[test]
    fn plain_http_credentials_and_junk_are_refused() {
        for input in [
            "http://192.168.77.20:2283",
            "http://immich.lan:2283",
            "http://127.0.0.1.nip.io",
        ] {
            let err = server_url(input).unwrap_err();
            assert!(err.contains("https://"), "{input}: {err}");
        }
        let err = server_url("https://admin:hunter2@photos.example.com").unwrap_err();
        assert!(!err.contains("hunter2"), "{err}");
        for input in [
            "",
            "ftp://photos.example.com",
            "https://photos.example.com/?k=1",
            "https://photos.example.com:99999",
        ] {
            assert!(server_url(input).is_err(), "{input}");
        }
    }

    #[test]
    fn library_ids_are_uuids() {
        assert!(valid_library_id(LIB));
        assert!(valid_library_id(&LIB.to_uppercase()));
        for bad in [
            "",
            "1",
            "../../users",
            &LIB.replace('-', "x"),
            &format!("{LIB}0"),
        ] {
            assert!(!valid_library_id(bad), "{bad}");
        }
    }

    #[test]
    fn import_paths_compare_by_folder() {
        assert_eq!(clean_path(" /mnt//photos/ ").as_deref(), Ok("/mnt/photos"));
        assert_eq!(clean_path("/").as_deref(), Ok("/"));
        assert!(clean_path("mnt/photos").is_err());
        assert!(clean_path("/mnt/../etc").is_err());
        assert!(covers("/mnt/photos", "/mnt/photos"));
        assert!(covers("/mnt/photos/", "/mnt/photos/2024/Holidays"));
        assert!(covers("/", "/mnt/photos"));
        assert!(!covers("/mnt/photos", "/mnt/photos-old"));
        assert!(!covers("/mnt/photos/2024", "/mnt/photos"));
        assert!(!covers("relative", "/mnt/photos"));
    }

    #[test]
    fn a_consistent_profile_passes() {
        let s = settings();
        s.check(&sftp("/srv/photos/Holidays")).unwrap();
        s.check(&Destination {
            backend: Backend::Local,
            ..sftp("/srv/photos")
        })
        .unwrap();
    }

    #[test]
    fn inconsistent_profiles_are_refused() {
        let s = settings();
        assert!(s.check(&sftp("srv/photos")).is_err());
        let webdav = Destination {
            backend: Backend::Webdav {
                url: "https://cloud.example.com/remote.php/dav/files/alice/".into(),
                user: "alice".into(),
                password: None,
                vendor: None,
            },
            ..sftp("Photos")
        };
        assert!(s.check(&webdav).is_err());
        for bad in [
            ImmichSettings {
                server: "photos.example.com".into(),
                ..settings()
            },
            ImmichSettings {
                library_id: "../admin".into(),
                ..settings()
            },
            ImmichSettings {
                import_path: "/mnt/photos/".into(),
                ..settings()
            },
            ImmichSettings {
                api_key: KeyringRef(String::new()),
                ..settings()
            },
        ] {
            assert!(bad.check(&sftp("/srv/photos")).is_err(), "{bad:?}");
        }
    }
}
