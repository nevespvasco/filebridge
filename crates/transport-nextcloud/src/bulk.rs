//! Nextcloud's bulk upload on the wire: where it lives, the request body and the answer.
//!
//! One `POST /remote.php/dav/bulk` carries many files as a `multipart/related` body. Each part
//! names its file by `X-File-Path` (from the user's files root, as raw UTF-8: `%`, `#` and
//! non-ASCII arrive as written), with `X-File-MD5` (the server refuses the whole request when one
//! doesn't match), `X-File-Mtime` (seconds) and `Content-Length`. The answer is a JSON object
//! keyed by those paths: `{"error": false, …}` or `{"error": true, "message": …}`. The server
//! advertises it as `dav.bulkupload` in its capabilities; parent folders must exist.

use std::collections::HashMap;
use std::fmt::Write as _;

use filebridge_transport_api::{Result, TransportError};
use serde_json::Value;

/// Where a Nextcloud's endpoints are, from a profile's WebDAV address
/// (`https://cloud.example.com/remote.php/dav/files/alice/`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Site {
    /// Everything before `/remote.php/`: scheme, host, port and any prefix the server lives
    /// under. Also what bulk support is remembered by.
    pub base: String,
    /// The user's files root, ending with `/`.
    pub files: String,
}

impl Site {
    /// `None` for an address that isn't a Nextcloud user's WebDAV files root.
    pub(crate) fn of(url: &str) -> Option<Self> {
        const FILES: &str = "/remote.php/dav/files/";
        let at = url.find(FILES)?;
        let user = &url[at + FILES.len()..];
        if user.trim_end_matches('/').is_empty() || user.trim_end_matches('/').contains('/') {
            return None;
        }
        let files = if url.ends_with('/') {
            url.to_owned()
        } else {
            format!("{url}/")
        };
        Some(Self {
            base: url[..at].to_owned(),
            files,
        })
    }

    pub(crate) fn bulk(&self) -> String {
        format!("{}/remote.php/dav/bulk", self.base)
    }

    pub(crate) fn capabilities(&self) -> String {
        format!("{}/ocs/v2.php/cloud/capabilities?format=json", self.base)
    }

    /// The WebDAV address of `path` (relative to the files root), each segment encoded.
    pub(crate) fn folder(&self, path: &str) -> String {
        let mut out = self.files.clone();
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            out.push_str(&encode_segment(segment));
            out.push('/');
        }
        out
    }
}

/// Whether a capabilities answer (`/ocs/v2.php/cloud/capabilities?format=json`) offers bulk
/// upload.
pub(crate) fn offers_bulk(v: &Value) -> bool {
    v.pointer("/ocs/data/capabilities/dav/bulkupload")
        .and_then(Value::as_str)
        .is_some_and(|version| version.starts_with("1."))
}

/// `root` and a destination path joined, as an `X-File-Path`.
pub(crate) fn file_path(root: &str, dest: &str) -> String {
    let root = root.trim_matches('/');
    if root.is_empty() {
        format!("/{dest}")
    } else {
        format!("/{root}/{dest}")
    }
}

/// The folders to create, top down, so that `folder` (relative to `root`) exists: every
/// ancestor from the files root, as paths relative to it.
pub(crate) fn folder_chain(root: &str, folder: Option<&str>) -> Vec<String> {
    let mut chain = Vec::new();
    let mut path = String::new();
    let segments = root
        .split('/')
        .chain(folder.into_iter().flat_map(|f| f.split('/')));
    for segment in segments.filter(|s| !s.is_empty()) {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(segment);
        chain.push(path.clone());
    }
    chain
}

/// A path segment percent-encoded for a URL: unreserved ASCII stays, every other byte is `%XX`.
fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for b in segment.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// One file of a bulk request.
pub(crate) struct Part {
    /// `X-File-Path`.
    pub path: String,
    pub data: Vec<u8>,
    /// Seconds since the Unix epoch.
    pub mtime: i64,
}

/// A boundary that no part's bytes contain.
pub(crate) fn boundary(parts: &[Part]) -> Result<String> {
    loop {
        let mut raw = [0u8; 16];
        getrandom::fill(&mut raw).map_err(|e| TransportError::fatal(format!("getrandom: {e}")))?;
        let b: String = raw.iter().map(|x| format!("{x:02x}")).collect();
        let b = format!("filebridge-{b}");
        if !parts
            .iter()
            .any(|p| p.data.windows(b.len()).any(|w| w == b.as_bytes()))
        {
            return Ok(b);
        }
    }
}

/// The `multipart/related` body for `parts`.
pub(crate) fn body(parts: &[Part], boundary: &str) -> Vec<u8> {
    let size: usize = parts
        .iter()
        .map(|p| p.data.len() + p.path.len() + 160)
        .sum();
    let mut out = Vec::with_capacity(size + boundary.len() + 8);
    for p in parts {
        let md5 = format!("{:x}", md5::compute(&p.data));
        let head = format!(
            "--{boundary}\r\nX-File-Path: {}\r\nX-File-MD5: {md5}\r\nX-File-Mtime: {}\r\n\
             Content-Length: {}\r\n\r\n",
            p.path,
            p.mtime,
            p.data.len()
        );
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(&p.data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// What the server said about each file, by `X-File-Path`: `None` when it was stored,
/// `Some(message)` when it wasn't.
pub(crate) fn answers(v: &Value) -> HashMap<String, Option<String>> {
    v.as_object()
        .into_iter()
        .flatten()
        .map(|(path, a)| {
            let failed = a.get("error").and_then(Value::as_bool) != Some(false);
            let message = a
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("not stored")
                .to_owned();
            (path.clone(), failed.then_some(message))
        })
        .collect()
}

/// The `PROPFIND` body asking for a folder's quota (RFC 4331).
pub(crate) const QUOTA_PROPFIND: &str = "<?xml version=\"1.0\"?>\
<d:propfind xmlns:d=\"DAV:\"><d:prop><d:quota-available-bytes/><d:quota-used-bytes/></d:prop>\
</d:propfind>";

/// A user's quota from a `PROPFIND` answer: `(available, used)` bytes. `None` when there is no
/// limit: Nextcloud answers a negative `quota-available-bytes` (`-3` unlimited, `-2` unknown,
/// `-1` not computed yet), or leaves it out.
pub(crate) fn quota(multistatus: &str) -> Option<(u64, u64)> {
    // `<d:quota-available-bytes>123</d:quota-available-bytes>`, any namespace prefix; the empty
    // `<d:quota-available-bytes/>` of a 404 propstat doesn't match.
    let value = |name: &str| -> Option<i64> {
        let at = multistatus.find(&format!("{name}>"))? + name.len() + 1;
        let rest = &multistatus[at..];
        rest[..rest.find('<')?].trim().parse().ok()
    };
    let available = u64::try_from(value("quota-available-bytes")?).ok()?;
    let used = value("quota-used-bytes")
        .and_then(|u| u64::try_from(u).ok())
        .unwrap_or(0);
    Some((available, used))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_site_comes_from_a_user_files_address() {
        let s = Site::of("https://cloud.example.com/nc/remote.php/dav/files/alice").unwrap();
        assert_eq!(s.base, "https://cloud.example.com/nc");
        assert_eq!(
            s.files,
            "https://cloud.example.com/nc/remote.php/dav/files/alice/"
        );
        assert_eq!(s.bulk(), "https://cloud.example.com/nc/remote.php/dav/bulk");
        assert_eq!(
            s.folder("Fotos/Ver\u{e3}o 2019/#1"),
            "https://cloud.example.com/nc/remote.php/dav/files/alice/Fotos/Ver%C3%A3o%202019/%231/"
        );
        for not in [
            "https://cloud.example.com/remote.php/webdav/",
            "https://cloud.example.com/remote.php/dav/files/",
            "https://cloud.example.com/remote.php/dav/files/alice/Fotos/",
        ] {
            assert_eq!(Site::of(not), None, "{not}");
        }
    }

    #[test]
    fn capabilities_say_whether_bulk_is_there() {
        let with =
            json!({ "ocs": { "data": { "capabilities": { "dav": { "bulkupload": "1.0" } } } } });
        assert!(offers_bulk(&with));
        assert!(!offers_bulk(
            &json!({ "ocs": { "data": { "capabilities": { "dav": {} } } } })
        ));
    }

    #[test]
    fn paths_and_folders_are_from_the_files_root() {
        assert_eq!(
            file_path("/Backup/2026/", "a/b.txt"),
            "/Backup/2026/a/b.txt"
        );
        assert_eq!(file_path("", "b.txt"), "/b.txt");
        assert_eq!(
            folder_chain("Backup/2026", Some("a/b")),
            ["Backup", "Backup/2026", "Backup/2026/a", "Backup/2026/a/b"]
        );
        assert_eq!(folder_chain("", None), Vec::<String>::new());
    }

    #[test]
    fn the_body_carries_each_file_with_its_md5() {
        let parts = vec![
            Part {
                path: "/x/Ver\u{e3}o.txt".into(),
                data: b"abc".to_vec(),
                mtime: 1_700_000_000,
            },
            Part {
                path: "/x/empty".into(),
                data: Vec::new(),
                mtime: 1,
            },
        ];
        let b = boundary(&parts).unwrap();
        let body = String::from_utf8(body(&parts, &b)).unwrap();
        assert!(body.starts_with(&format!("--{b}\r\nX-File-Path: /x/Ver\u{e3}o.txt\r\n")));
        assert!(body.contains("X-File-MD5: 900150983cd24fb0d6963f7d28e17f72\r\n"));
        assert!(body.contains("X-File-Mtime: 1700000000\r\nContent-Length: 3\r\n\r\nabc\r\n"));
        assert!(body.contains("Content-Length: 0\r\n\r\n\r\n"));
        assert!(body.ends_with(&format!("--{b}--\r\n")));
    }

    #[test]
    fn answers_are_per_path() {
        let a = answers(&json!({
            "/a.txt": { "error": false, "etag": "e" },
            "/b.txt": { "error": true, "message": "Could not create path" },
        }));
        assert_eq!(a["/a.txt"], None);
        assert_eq!(a["/b.txt"].as_deref(), Some("Could not create path"));
    }

    #[test]
    fn quota_from_a_propfind_answer() {
        let answer = |available: &str| {
            format!(
                "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\" xmlns:s=\"http://sabredav.org/ns\">\
                 <d:response><d:href>/remote.php/dav/files/alice/</d:href><d:propstat><d:prop>\
                 <d:quota-available-bytes>{available}</d:quota-available-bytes>\
                 <d:quota-used-bytes>524288000</d:quota-used-bytes>\
                 </d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"
            )
        };
        assert_eq!(quota(&answer("1048576")), Some((1_048_576, 524_288_000)));
        assert_eq!(quota(&answer("-3")), None, "unlimited");
        assert_eq!(quota(&answer("-2")), None, "unknown");
        let missing = "<d:multistatus xmlns:d=\"DAV:\"><d:response><d:propstat><d:prop>\
            <d:quota-available-bytes/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status>\
            </d:propstat></d:response></d:multistatus>";
        assert_eq!(quota(missing), None);
        assert_eq!(
            quota(&answer("1024").replace("d:", "D:")),
            Some((1024, 524_288_000))
        );
    }
}
