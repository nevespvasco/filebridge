//! [`MockImmich`]: a small HTTP/1.1 server on `127.0.0.1` that answers the calls this
//! integration makes like Immich does, and records them. For the tests here and in the hosts
//! (feature `fake`).

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
        });
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
        "assetCount": 42,
        "importPaths": l.import_paths,
        "exclusionPatterns": ["**/@eaDir/**", "**/._*"],
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

fn answer(state: &mut State, req: &Recorded) -> (u16, Value) {
    let segments: Vec<&str> = req.path.trim_start_matches('/').split('/').collect();
    match (req.method.as_str(), segments.as_slice()) {
        ("GET", ["api", "server", "ping"]) => return (200, json!({ "res": "pong" })),
        ("GET", ["api", "server", "version"]) => {
            return (200, json!({ "major": 1, "minor": 132, "patch": 3 }));
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
                    .into_iter()
                    .map(|p| {
                        if l.readable {
                            json!({ "importPath": p, "isValid": true })
                        } else {
                            json!({ "importPath": p, "isValid": false, "message": "Path does not exist (ENOENT)" })
                        }
                    })
                    .collect();
                (200, json!({ "importPaths": paths }))
            }
            None => error(400, "Library not found"),
        },
        ("POST", ["api", "libraries", id, "scan"]) => match (find(id), state.scan_status) {
            (None, _) => error(400, "Library not found"),
            (Some(_), Some(status)) if status >= 300 => error(status, "scan refused"),
            (Some(_), _) => (204, Value::Null),
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
