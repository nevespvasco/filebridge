//! [`MockImmich`]: a small HTTP/1.1 server on `127.0.0.1` that answers the calls this
//! integration makes like Immich does, and records them. For the tests here and in the hosts
//! (feature `fake`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One request the mock received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// The `x-api-key` header, if any.
    pub api_key: Option<String>,
    pub body: String,
}

/// A library the mock serves, and whether Immich "can read" its import paths.
#[derive(Debug, Clone)]
pub struct MockLibrary {
    pub id: String,
    pub name: String,
    pub import_paths: Vec<String>,
    pub readable: bool,
    pub exclusion_patterns: Vec<String>,
    pub asset_count: u64,
}

/// Which queue API the mock offers, like the Immich versions do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueueApi {
    /// `GET /api/queues/library` (Immich 2.x; `/api/jobs` answers too, deprecated).
    #[default]
    Current,
    /// Only `GET /api/jobs` (older Immich).
    Legacy,
    /// Neither: every queue call answers this status.
    Status(u16),
}

#[derive(Debug, Default)]
struct State {
    key: String,
    /// The key's user is an administrator (else library calls answer 403).
    admin: bool,
    libraries: Vec<MockLibrary>,
    /// Status for scan requests (default 204).
    scan_status: Option<u16>,
    requests: Vec<Recorded>,
    queue_api: QueueApi,
    /// A scan keeps the library queue busy for this many looks at it (default 2).
    scan_polls: u32,
    /// Looks left before the running scan finishes; and the library it scans.
    running: Option<(u32, String)>,
    queue_paused: bool,
    /// Assets each finished scan adds to its library.
    imported_per_scan: u64,
    /// `/server/media-types` answers 404, like an older Immich.
    no_media_types: bool,
    /// What Immich's file system holds: `(path as Immich sees it, the folder here)`.
    mounts: Vec<(String, PathBuf)>,
}

/// The server. Dropping it stops answering new connections.
pub struct MockImmich {
    url: String,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockImmich {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockImmich {
    /// Serves Immich with one valid `key`, an administrator's.
    pub async fn start(key: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let state = Arc::new(Mutex::new(State {
            key: key.into(),
            admin: true,
            scan_polls: 2,
            ..State::default()
        }));
        let shared = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = shared.clone();
                tokio::spawn(async move {
                    let _ = serve(stream, state).await;
                });
            }
        });
        Self { url, state, task }
    }

    /// `http://127.0.0.1:<port>`, what a profile's `server` holds.
    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn add_library(&self, id: &str, name: &str, import_paths: &[&str]) {
        self.state.lock().unwrap().libraries.push(MockLibrary {
            id: id.into(),
            name: name.into(),
            import_paths: import_paths.iter().map(|p| (*p).to_owned()).collect(),
            readable: true,
            exclusion_patterns: vec!["**/@eaDir/**".into(), "**/._*".into()],
            asset_count: 42,
        });
    }

    /// The library's exclusion patterns (default `**/@eaDir/**` and `**/._*`).
    pub fn set_exclusion_patterns(&self, id: &str, patterns: &[&str]) {
        for l in &mut self.state.lock().unwrap().libraries {
            if l.id == id {
                l.exclusion_patterns = patterns.iter().map(|p| (*p).to_owned()).collect();
            }
        }
    }

    pub fn set_queue_api(&self, api: QueueApi) {
        self.state.lock().unwrap().queue_api = api;
    }

    /// A scan keeps the library queue busy for `polls` looks at it.
    pub fn set_scan_polls(&self, polls: u32) {
        self.state.lock().unwrap().scan_polls = polls;
    }

    /// The library queue is paused: a scan never finishes.
    pub fn set_queue_paused(&self, paused: bool) {
        self.state.lock().unwrap().queue_paused = paused;
    }

    /// Each finished scan adds `n` assets to its library.
    pub fn set_imported_per_scan(&self, n: u64) {
        self.state.lock().unwrap().imported_per_scan = n;
    }

    /// A scan is running, as if asked for before (by an earlier run of the step).
    pub fn start_scan(&self, id: &str, polls: u32) {
        self.state.lock().unwrap().running = Some((polls, id.into()));
    }

    pub fn set_no_media_types(&self) {
        self.state.lock().unwrap().no_media_types = true;
    }

    /// Immich sees the folder `here` as `seen_as` (the path validation looks there). Without
    /// any, every path validates as Immich can read it.
    pub fn mount(&self, seen_as: &str, here: impl Into<PathBuf>) {
        self.state
            .lock()
            .unwrap()
            .mounts
            .push((seen_as.trim_end_matches('/').to_owned(), here.into()));
    }

    /// Immich can't read the import paths of library `id` (validate says so).
    pub fn set_unreadable(&self, id: &str) {
        for l in &mut self.state.lock().unwrap().libraries {
            if l.id == id {
                l.readable = false;
            }
        }
    }

    pub fn set_admin(&self, admin: bool) {
        self.state.lock().unwrap().admin = admin;
    }

    /// Answer scans with `status` instead of `204`.
    pub fn set_scan_status(&self, status: u16) {
        self.state.lock().unwrap().scan_status = Some(status);
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.state.lock().unwrap().requests.clone()
    }

    /// The paths of the scan requests received.
    pub fn scans(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == "POST" && r.path.ends_with("/scan"))
            .map(|r| r.path)
            .collect()
    }
}

fn library_json(l: &MockLibrary) -> Value {
    json!({
        "id": l.id,
        "name": l.name,
        "ownerId": "0b0c7f8e-1111-4222-8333-944455556666",
        "assetCount": l.asset_count,
        "importPaths": l.import_paths,
        "exclusionPatterns": l.exclusion_patterns,
        "createdAt": "2026-01-01T00:00:00.000Z",
        "updatedAt": "2026-01-01T00:00:00.000Z",
        "refreshedAt": null,
    })
}

fn error(status: u16, message: &str) -> (u16, Value) {
    (
        status,
        json!({ "message": message, "statusCode": status, "correlationId": "mock" }),
    )
}

/// What `POST /validate` says about `path`, like Immich's `validateImportPath`.
fn validate_one(state: &State, l: &MockLibrary, path: &str) -> Value {
    let invalid =
        |message: &str| json!({ "importPath": path, "isValid": false, "message": message });
    if !l.readable {
        return invalid("Path does not exist (ENOENT)");
    }
    let mounted = state.mounts.iter().find_map(|(seen, here)| {
        let rest = path.strip_prefix(seen.as_str())?;
        (rest.is_empty() || rest.starts_with('/')).then(|| here.join(rest.trim_start_matches('/')))
    });
    match mounted {
        Some(local) => match std::fs::metadata(local) {
            Ok(m) if m.is_dir() => json!({ "importPath": path, "isValid": true }),
            Ok(_) => invalid("Not a directory"),
            Err(_) => invalid("Path does not exist (ENOENT)"),
        },
        None if state.mounts.is_empty() => json!({ "importPath": path, "isValid": true }),
        None => invalid("Path does not exist (ENOENT)"),
    }
}

/// One look at the library queue: a running scan gets one look closer to its end.
fn look_at_queue(state: &mut State) -> Value {
    let (active, waiting) = match &mut state.running {
        Some((left, _)) if state.queue_paused => (0, u64::from(*left)),
        Some((left, id)) => {
            *left = left.saturating_sub(1);
            if *left == 0 {
                let id = id.clone();
                state.running = None;
                let n = state.imported_per_scan;
                if let Some(l) = state.libraries.iter_mut().find(|l| l.id == id) {
                    l.asset_count += n;
                }
                (0, 0)
            } else {
                (1, u64::from(*left - 1))
            }
        }
        None => (0, 0),
    };
    json!({
        "active": active, "completed": 7, "failed": 0, "delayed": 0, "waiting": waiting,
        "paused": 0,
    })
}

fn answer(state: &mut State, req: &Recorded) -> (u16, Value) {
    let segments: Vec<&str> = req.path.trim_start_matches('/').split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["api", "server", "ping"]) => return (200, json!({ "res": "pong" })),
        ("GET", ["api", "server", "version"]) => {
            return (200, json!({ "major": 1, "minor": 132, "patch": 3 }));
        }
        ("GET", ["api", "server", "media-types"]) if state.no_media_types => {
            return error(404, "Not found");
        }
        ("GET", ["api", "server", "media-types"]) => {
            return (
                200,
                json!({
                    "image": [".jpg", ".jpeg", ".heic", ".png", ".dng"],
                    "video": [".mp4", ".mov"],
                    "sidecar": [".xmp"],
                }),
            );
        }
        _ => {}
    }
    if req.api_key.as_deref() != Some(state.key.as_str()) {
        return error(401, "Invalid API key");
    }
    if !state.admin {
        return error(403, "Forbidden");
    }
    let find = |id: &str| state.libraries.iter().find(|l| l.id == id).cloned();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["api", "libraries"]) => (
            200,
            Value::Array(state.libraries.iter().map(library_json).collect()),
        ),
        ("GET", ["api", "libraries", id]) => match find(id) {
            Some(l) => (200, library_json(&l)),
            None => error(400, "Library not found"),
        },
        ("POST", ["api", "libraries", id, "validate"]) => match find(id) {
            Some(l) => {
                let body: Value = serde_json::from_str(&req.body).unwrap_or(Value::Null);
                let paths: Vec<Value> = body["importPaths"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .map(|p| validate_one(state, &l, p.as_str().unwrap_or_default()))
                    .collect();
                (200, json!({ "importPaths": paths }))
            }
            None => error(400, "Library not found"),
        },
        ("POST", ["api", "libraries", id, "scan"]) => match (find(id), state.scan_status) {
            (None, _) => error(400, "Library not found"),
            (Some(_), Some(status)) if status >= 300 => error(status, "scan refused"),
            (Some(_), _) => {
                state.running = Some((state.scan_polls.max(1), id.to_string()));
                (204, Value::Null)
            }
        },
        ("GET", ["api", "queues", "library"]) => match state.queue_api {
            QueueApi::Current => {
                let paused = state.queue_paused;
                let stats = look_at_queue(state);
                (
                    200,
                    json!({ "name": "library", "isPaused": paused, "statistics": stats }),
                )
            }
            QueueApi::Legacy => error(404, "Not found"),
            QueueApi::Status(s) => error(s, "queue refused"),
        },
        ("GET", ["api", "jobs"]) => match state.queue_api {
            QueueApi::Status(s) => error(s, "queue refused"),
            _ => {
                let paused = state.queue_paused;
                let stats = look_at_queue(state);
                let other = json!({
                    "jobCounts": { "active": 3, "completed": 0, "failed": 0, "delayed": 0,
                                   "waiting": 9, "paused": 0 },
                    "queueStatus": { "isActive": true, "isPaused": false },
                });
                (
                    200,
                    json!({
                        "library": {
                            "jobCounts": stats,
                            "queueStatus": { "isActive": stats["active"] != 0, "isPaused": paused },
                        },
                        "thumbnailGeneration": other,
                    }),
                )
            }
        },
        _ => error(404, "Not found"),
    }
}

async fn serve(mut stream: TcpStream, state: Arc<Mutex<State>>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next().unwrap_or_default().to_owned();
    let path = first.next().unwrap_or_default().to_owned();
    let mut length = 0usize;
    let mut api_key = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap_or(0),
                "x-api-key" => api_key = Some(value.trim().to_owned()),
                _ => {}
            }
        }
    }
    while buf.len() < head_end + length {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[head_end..]).into_owned();
    let req = Recorded {
        method,
        path,
        api_key,
        body,
    };
    let (status, value) = {
        let mut state = state.lock().unwrap();
        state.requests.push(req.clone());
        answer(&mut state, &req)
    };
    let body = if status == 204 {
        String::new()
    } else {
        value.to_string()
    };
    let reply = format!(
        "HTTP/1.1 {status} Mock\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(reply.as_bytes()).await?;
    stream.shutdown().await
}
