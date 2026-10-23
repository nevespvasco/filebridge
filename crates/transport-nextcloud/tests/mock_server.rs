//! `NextcloudBulk` against a small HTTP/1.1 server on `127.0.0.1` that answers like Nextcloud
//! (capabilities, `PROPFIND` quota, bulk upload), over `FakeTransport` as the inner transport
//! (it lists and copies into a local folder). Nothing leaves the machine.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use filebridge_secrets::{MemoryStore, SecretKind, SecretStore, StoreResolver, reference};
use filebridge_transport_api::{
    Backend, Destination, ErrorKind, ItemOutcome, ProgressEvent, ProgressSink, RemotePath,
    TransferItem, TransferJob, TransferOptions, Transport,
};
use filebridge_transport_fake::FakeTransport;
use filebridge_transport_nextcloud::NextcloudBulk;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// How the mock answers.
#[derive(Clone)]
struct Answers {
    /// Status of `POST /remote.php/dav/bulk`.
    bulk: u16,
    /// `quota-available-bytes` (`-3`: unlimited) and `quota-used-bytes`.
    quota: (i64, u64),
}

struct MockNextcloud {
    url: String,
    requests: Arc<Mutex<Vec<(String, String)>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockNextcloud {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockNextcloud {
    async fn start(answers: Answers) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (answers, seen) = (answers.clone(), seen.clone());
                tokio::spawn(async move { serve(stream, &answers, &seen).await });
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }

    fn requests(&self) -> Vec<(String, String)> {
        self.requests.lock().unwrap().clone()
    }
}

/// One request per connection (`Connection: close`).
async fn serve(mut stream: TcpStream, answers: &Answers, seen: &Mutex<Vec<(String, String)>>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let Ok(n) = stream.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or("").split(' ');
    let method = first.next().unwrap_or("").to_string();
    let path = first.next().unwrap_or("").to_string();
    let length: usize = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + length {
        let Ok(n) = stream.read(&mut chunk).await else {
            return;
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    seen.lock().unwrap().push((method.clone(), path.clone()));
    let (status, body) = match (method.as_str(), path.as_str()) {
        ("GET", p) if p.starts_with("/ocs/v2.php/cloud/capabilities") => (
            200,
            r#"{"ocs":{"meta":{"status":"ok"},"data":{"capabilities":{"dav":{"bulkupload":"1.0"}}}}}"#
                .to_string(),
        ),
        ("PROPFIND", "/remote.php/dav/files/alice/") => (
            207,
            format!(
                "<?xml version=\"1.0\"?><d:multistatus xmlns:d=\"DAV:\"><d:response>\
                 <d:href>/remote.php/dav/files/alice/</d:href><d:propstat><d:prop>\
                 <d:quota-available-bytes>{}</d:quota-available-bytes>\
                 <d:quota-used-bytes>{}</d:quota-used-bytes></d:prop>\
                 <d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>",
                answers.quota.0, answers.quota.1
            ),
        ),
        ("POST", "/remote.php/dav/bulk") if answers.bulk == 507 => (
            507,
            "<?xml version=\"1.0\"?><d:error xmlns:d=\"DAV:\" xmlns:s=\"http://sabredav.org/ns\">\
             <s:exception>Sabre\\DAV\\Exception\\InsufficientStorage</s:exception>\
             <s:message>Insufficient space in /alice/files</s:message></d:error>"
                .to_string(),
        ),
        ("MKCOL", _) => (201, String::new()),
        _ => (404, String::new()),
    };
    let reason = match status {
        200 => "OK",
        201 => "Created",
        207 => "Multi-Status",
        507 => "Insufficient Storage",
        _ => "Not Found",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// The transport under test over a `FakeTransport` that writes into `dest_dir`, and the
/// Nextcloud destination whose root is that folder.
fn bulk(
    server: &str,
    dest_dir: &std::path::Path,
) -> (NextcloudBulk, Arc<FakeTransport>, Destination) {
    let store = Arc::new(MemoryStore::default());
    let pass_ref = reference("p-nextcloud", SecretKind::AppPassword);
    store.set(&pass_ref, "app-password-fixture").unwrap();
    let inner = Arc::new(FakeTransport::default());
    let transport = NextcloudBulk::over(inner.clone(), Arc::new(StoreResolver(store)));
    let dest = Destination {
        transport: "fake".into(),
        backend: Backend::Webdav {
            url: format!("{server}/remote.php/dav/files/alice/"),
            user: "alice".into(),
            password: Some(pass_ref),
            vendor: Some("nextcloud".into()),
        },
        // The fake lists and writes here; for Nextcloud it is the folder inside alice's files.
        root: dest_dir.to_string_lossy().into_owned(),
    };
    (transport, inner, dest)
}

fn small_files(dir: &std::path::Path, n: usize) -> (PathBuf, Vec<TransferItem>) {
    let src = dir.join("src");
    std::fs::create_dir_all(&src).unwrap();
    let items = (0..n)
        .map(|i| {
            let name = format!("note-{i}.txt");
            std::fs::write(src.join(&name), format!("note {i}")).unwrap();
            TransferItem {
                root: 0,
                source: PathBuf::from(&name),
                dest: RemotePath::new(name).unwrap(),
                size: format!("note {i}").len() as u64,
                expected_hash: None,
            }
        })
        .collect();
    (src, items)
}

#[tokio::test]
async fn a_bulk_upload_answered_507_fails_its_files_full_without_resending_them() {
    let server = MockNextcloud::start(Answers {
        bulk: 507,
        quota: (-3, 0),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let dest_dir = dir.path().join("dest");
    std::fs::create_dir_all(&dest_dir).unwrap();
    let (transport, inner, dest) = bulk(&server.url, &dest_dir);
    let (src, items) = small_files(dir.path(), 3);
    let job = TransferJob {
        id: "bulk-507".into(),
        source_roots: vec![src],
        items,
        options: TransferOptions { concurrency: 4 },
    };
    let (sink, mut rx) = ProgressSink::channel();
    let sent = transport.transfer(&dest, &job, sink).await.unwrap();

    assert_eq!(sent.failed(), 3, "{:?}", sent.items);
    for outcome in &sent.items {
        let ItemOutcome::Failed(e) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(e.kind, ErrorKind::Full, "{e:?}");
        assert!(
            !e.is_retryable(),
            "a full destination pauses the job, not a retry"
        );
    }
    // Not sent again through the inner transport: it would only be refused again.
    assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read_dir(&dest_dir).unwrap().count(), 0);
    let bulk_posts = server
        .requests()
        .iter()
        .filter(|(m, p)| m == "POST" && p == "/remote.php/dav/bulk")
        .count();
    assert_eq!(bulk_posts, 1, "{:?}", server.requests());
    let mut failed = 0;
    while let Ok(event) = rx.try_recv() {
        if let ProgressEvent::ItemDone { ok: false, .. } = event {
            failed += 1;
        }
    }
    assert_eq!(failed, 3, "ItemDone for every file");
}

#[tokio::test]
async fn the_quota_is_the_free_space_when_it_is_tighter() {
    let server = MockNextcloud::start(Answers {
        bulk: 200,
        quota: (1_000_000, 4_000_000),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (transport, _, dest) = bulk(&server.url, dir.path());
    let report = transport.preflight(&dest).await.unwrap();
    assert_eq!(report.free_bytes, Some(1_000_000));
    assert_eq!(report.used_bytes, Some(4_000_000));
    assert_eq!(report.total_bytes, Some(5_000_000));
    assert!(report.checks.iter().any(|c| c.id == "quota"));
    assert!(
        server
            .requests()
            .iter()
            .any(|(m, p)| m == "PROPFIND" && p == "/remote.php/dav/files/alice/")
    );
}

#[tokio::test]
async fn no_quota_leaves_the_inner_report() {
    let server = MockNextcloud::start(Answers {
        bulk: 200,
        quota: (-3, 123),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let (transport, inner, dest) = bulk(&server.url, dir.path());
    *inner.free.lock().unwrap() = Some(77);
    let report = transport.preflight(&dest).await.unwrap();
    assert_eq!(report.free_bytes, Some(77));
    assert!(!report.checks.iter().any(|c| c.id == "quota"));
}
