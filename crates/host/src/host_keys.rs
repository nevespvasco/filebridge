//! Trusting an SSH server's host key from the profile wizards (ADR 0015, docs/07):
//! `scan_host_key` shows the key of a server `known_hosts` doesn't know yet, and
//! `trust_host_key` adds it once the user has confirmed its fingerprint. A changed key is
//! refused with `host_key_changed`, never replaced.

use std::path::PathBuf;

use filebridge_integration_api::IntegrationError;
use filebridge_integration_nextcloud::known_hosts::{self, Standing};
use filebridge_integration_nextcloud::ssh::{self, HostKeyScan, TrustError};
use filebridge_ipc::{ErrorBody, HostKeyStatus, HostKeyView};

type Res<T> = Result<T, ErrorBody>;

/// Unreachable servers stay `ssh` (try again); anything else about the check is `invalid`.
fn to_error(e: IntegrationError) -> ErrorBody {
    ErrorBody {
        code: if e.retryable { "ssh" } else { "invalid" }.into(),
        message: e.message,
    }
}

/// The address the user typed, checked before any connection.
fn checked(host: &str, port: u16, file: Option<String>) -> Res<(String, Option<PathBuf>)> {
    let host = host.trim();
    if host.is_empty() || port == 0 {
        return Err(ErrorBody {
            code: "invalid".into(),
            message: "an SSH server needs a host and a port".into(),
        });
    }
    let file = file
        .map(|f| f.trim().to_owned())
        .filter(|f| !f.is_empty())
        .map(PathBuf::from);
    if let Some(f) = &file
        && !f.is_absolute()
    {
        return Err(ErrorBody {
            code: "invalid".into(),
            message: format!("known_hosts file {} must be a full path", f.display()),
        });
    }
    Ok((host.to_owned(), file))
}

fn view(host: &str, port: u16, scan: HostKeyScan) -> HostKeyView {
    let (status, line) = match scan.standing {
        Standing::Known => (HostKeyStatus::Known, None),
        Standing::Unknown => (HostKeyStatus::Unknown, None),
        Standing::Changed { line } => (HostKeyStatus::Changed, u32::try_from(line).ok()),
    };
    HostKeyView {
        host: host.to_owned(),
        port,
        known_hosts_file: scan.known_hosts.display().to_string(),
        status,
        key_type: scan.key.as_ref().map(|k| k.algorithm().to_string()),
        fingerprint: scan.key.as_ref().map(known_hosts::fingerprint),
        line,
    }
}

pub(crate) async fn scan(host: String, port: u16, file: Option<String>) -> Res<HostKeyView> {
    let (host, file) = checked(&host, port, file)?;
    let scan = ssh::scan_host_key(&host, port, file.as_deref())
        .await
        .map_err(to_error)?;
    Ok(view(&host, port, scan))
}

pub(crate) async fn trust(
    host: String,
    port: u16,
    file: Option<String>,
    fingerprint: String,
) -> Res<HostKeyView> {
    let (host, file) = checked(&host, port, file)?;
    let scan = ssh::trust_host_key(&host, port, file.as_deref(), &fingerprint)
        .await
        .map_err(|e| match e {
            TrustError::Refused(message) => ErrorBody {
                code: "host_key_changed".into(),
                message,
            },
            TrustError::Failed(e) => to_error(e),
        })?;
    tracing::info!(host, port, file = %scan.known_hosts.display(), "host key trusted");
    Ok(view(&host, port, scan))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_address_is_checked_before_connecting() {
        assert_eq!(checked(" ", 22, None).unwrap_err().code, "invalid");
        assert_eq!(checked("nas.example", 0, None).unwrap_err().code, "invalid");
        let e = checked("nas.example", 22, Some("~/.ssh/known_hosts".into())).unwrap_err();
        assert!(e.message.contains("full path"), "{}", e.message);
        assert_eq!(
            checked(" nas.example ", 22, Some("  ".into())).unwrap(),
            ("nas.example".to_owned(), None)
        );
    }
}
