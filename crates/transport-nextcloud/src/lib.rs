//! A [`Transport`] for Nextcloud destinations that sends small files through Nextcloud's bulk
//! upload (one request for up to [`MAX_REQUEST_FILES`] files) and everything else through
//! another transport, rclone in the app (ADR 0016).
//!
//! It answers to the inner transport's id and passes every other method to it, so profiles keep
//! their transport and nothing changes for destinations that aren't a Nextcloud with bulk upload
//! (the server's capabilities say, once per server).
//!
//! A send of a batch to a Nextcloud with bulk upload:
//! 1. Files up to [`MAX_FILE`] bytes are candidates. Their folders are listed once (through the
//!    inner transport): a file already there with the same size and modification time is done
//!    without sending (as rclone skips it); one that answers to the name only in another
//!    normalisation or case, or a folder that can't be listed, goes to the inner transport.
//!    Folders that aren't there are created (`MKCOL`, top down).
//! 2. Candidates go in requests of at most [`MAX_REQUEST_FILES`] files and
//!    [`MAX_REQUEST_BYTES`] bytes, up to [`PARALLEL_REQUESTS`] at a time (the job's concurrency
//!    if lower), under the app's bandwidth
//!    limit. A file the server didn't store, or a request that failed for anything but a refused
//!    sign-in, goes to the inner transport, which knows how to classify its errors.
//! 3. Everything left goes to the inner transport in one call; its progress is renumbered to
//!    the batch's items and added to what bulk already sent.

mod bulk;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use async_trait::async_trait;
use filebridge_names::loose_key;
use filebridge_transport_api::{
    Backend, Destination, Digest, ErrorKind, HashAlgo, InFlight, ItemOutcome, MoveRequest,
    PreflightReport, ProgressEvent, ProgressSink, RemoteEntry, RemotePath, Result, SecretResolver,
    SecretValue, StatsSnapshot, TransferJob, TransferResult, Transport, TransportCaps,
    TransportError,
};
use reqwest::StatusCode;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::bulk::{Part, Site};

/// Largest file sent by bulk upload; bigger ones go to the inner transport (chunked uploads,
/// resume).
pub const MAX_FILE: u64 = 4 << 20;
/// Most files in one bulk request.
pub const MAX_REQUEST_FILES: usize = 100;
/// Most bytes in one bulk request.
pub const MAX_REQUEST_BYTES: u64 = 16 << 20;
/// Most bulk requests in flight at once; fewer when the job's concurrency is lower. Five
/// requests of 100 files are a whole batch of the engine's default 500.
pub const PARALLEL_REQUESTS: usize = 5;

/// Small files to Nextcloud by bulk upload, the rest through `inner`.
pub struct NextcloudBulk {
    inner: Arc<dyn Transport>,
    secrets: Arc<dyn SecretResolver>,
    http: reqwest::Client,
    /// Whether a server (by [`Site::base`]) offers bulk upload, once asked.
    offers: Mutex<HashMap<String, bool>>,
    limit: Limiter,
}

impl NextcloudBulk {
    /// Bulk upload in front of `inner`, signing in with the profiles' secrets from `secrets`.
    pub fn over(inner: Arc<dyn Transport>, secrets: Arc<dyn SecretResolver>) -> Self {
        let builder = || {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(15 * 60))
        };
        // The OS's certificates and the bundled Mozilla roots; without the OS's when its store
        // can't be read (every one invalid), rather than no client at all.
        let http = builder()
            .build()
            .or_else(|_| builder().tls_built_in_native_certs(false).build())
            .expect("a client with the bundled roots always builds");
        Self {
            inner,
            secrets,
            http,
            offers: Mutex::new(HashMap::new()),
            limit: Limiter::default(),
        }
    }

    /// The server, user and password to send to by bulk upload; `None` when `dest` isn't a
    /// Nextcloud that offers it (or that can't be asked now).
    async fn signed_in(&self, dest: &Destination) -> Result<Option<SignedIn>> {
        let Backend::Webdav {
            url,
            user,
            password: Some(password),
            vendor: Some(vendor),
        } = &dest.backend
        else {
            return Ok(None);
        };
        let Some(site) = Site::of(url).filter(|_| vendor == "nextcloud") else {
            return Ok(None);
        };
        let known = self
            .offers
            .lock()
            .expect("offers lock")
            .get(&site.base)
            .copied();
        if known == Some(false) {
            return Ok(None);
        }
        let s = SignedIn {
            site,
            user: user.clone(),
            password: Arc::new(self.secrets.resolve(password).await?),
        };
        if known.is_none() {
            let answer = self
                .http
                .get(s.site.capabilities())
                .basic_auth(&s.user, Some(s.password.expose()))
                .header("OCS-APIRequest", "true")
                .send()
                .await;
            let offers = match answer {
                Ok(r) if r.status() == StatusCode::UNAUTHORIZED => {
                    return Err(refused(&s.site));
                }
                Ok(r) if r.status().is_success() => match r.json::<Value>().await {
                    Ok(v) => bulk::offers_bulk(&v),
                    Err(_) => return Ok(None),
                },
                Ok(r) if r.status().is_server_error() => return Ok(None),
                Ok(_) => false,
                // Can't tell now; the inner transport reports what is wrong.
                Err(_) => return Ok(None),
            };
            self.offers
                .lock()
                .expect("offers lock")
                .insert(s.site.base.clone(), offers);
            if !offers {
                return Ok(None);
            }
        }
        Ok(Some(s))
    }

    /// Step 1: which candidates are already there (done), which go to the inner transport, and
    /// the folders to create for the rest. `Err` only for a refused sign-in.
    async fn sort(
        &self,
        dest: &Destination,
        job: &TransferJob,
        candidates: Vec<Candidate>,
        s: &SignedIn,
        out: &mut Outcomes,
    ) -> Result<Vec<Candidate>> {
        let mut by_folder: HashMap<Option<String>, Vec<Candidate>> = HashMap::new();
        for c in candidates {
            let folder = job.items[c.index].dest.parent().map(str::to_owned);
            by_folder.entry(folder).or_default().push(c);
        }
        let mut to_send = Vec::new();
        let mut made: HashSet<String> = HashSet::new();
        for (folder, files) in by_folder {
            let dir = match folder.as_deref().map(RemotePath::new).transpose() {
                Ok(dir) => dir,
                Err(_) => {
                    out.to_inner.extend(files.iter().map(|c| c.index));
                    continue;
                }
            };
            let listed = match self.inner.list(dest, dir.as_ref(), false).await {
                Ok(entries) => Some(entries),
                Err(e) if e.kind == ErrorKind::NotFound => None,
                Err(e) if e.kind == ErrorKind::Auth => return Err(e),
                Err(_) => {
                    out.to_inner.extend(files.iter().map(|c| c.index));
                    continue;
                }
            };
            match listed {
                Some(entries) => {
                    for c in files {
                        let name = job.items[c.index].dest.file_name();
                        match there(&entries, name) {
                            There::No => to_send.push(c),
                            There::Exactly(e) if same_file(e, &c) => out.skipped(c.index),
                            There::Exactly(e) if !e.is_dir => to_send.push(c),
                            _ => out.to_inner.push(c.index),
                        }
                    }
                }
                None => {
                    match self
                        .make_folders(s, &dest.root, folder.as_deref(), &mut made)
                        .await
                    {
                        Ok(()) => to_send.extend(files),
                        Err(e) if e.kind == ErrorKind::Auth => return Err(e),
                        Err(_) => out.to_inner.extend(files.iter().map(|c| c.index)),
                    }
                }
            }
        }
        Ok(to_send)
    }

    /// Creates `folder` under `root` and every missing ancestor, top down.
    async fn make_folders(
        &self,
        s: &SignedIn,
        root: &str,
        folder: Option<&str>,
        made: &mut HashSet<String>,
    ) -> Result<()> {
        for path in bulk::folder_chain(root, folder) {
            if made.contains(&path) {
                continue;
            }
            let r = self
                .http
                .request(
                    reqwest::Method::from_bytes(b"MKCOL").expect("MKCOL is a valid method"),
                    s.site.folder(&path),
                )
                .basic_auth(&s.user, Some(s.password.expose()))
                .send()
                .await
                .map_err(|e| TransportError::retryable(format!("create folder {path}: {e}")))?;
            match r.status() {
                // Created, or already there.
                s if s.is_success() || s == StatusCode::METHOD_NOT_ALLOWED => {}
                StatusCode::UNAUTHORIZED => return Err(refused(&s.site)),
                status => {
                    return Err(TransportError::fatal(format!(
                        "create folder {path}: {status}"
                    )));
                }
            }
            made.insert(path);
        }
        Ok(())
    }

    /// Step 2: sends `files` by bulk upload. What arrived is reported as it does; what the
    /// server didn't take goes to `out.to_inner`. `Err` only for a refused sign-in.
    async fn send_bulk(
        &self,
        dest: &Destination,
        job: &TransferJob,
        files: Vec<Candidate>,
        s: &SignedIn,
        out: &mut Outcomes,
        progress: &Progress<'_>,
    ) -> Result<()> {
        let parallel = job.options.concurrency.clamp(1, PARALLEL_REQUESTS);
        let sem = Arc::new(Semaphore::new(parallel));
        let mut set = JoinSet::new();
        let mut in_flight: HashMap<usize, Vec<(usize, InFlight)>> = HashMap::new();
        let mut refusal = None;
        for (n, request) in requests(files).into_iter().enumerate() {
            let permit = sem
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            while let Some(done) = set.try_join_next() {
                if let Err(e) = self.finished(done, out, progress, &mut in_flight) {
                    refusal = Some(e);
                }
            }
            if refusal.is_some() {
                break;
            }
            self.limit
                .wait(request.iter().map(|c| c.size).sum::<u64>())
                .await;
            in_flight.insert(
                n,
                request
                    .iter()
                    .map(|c| {
                        let f = InFlight {
                            dest: job.items[c.index].dest.as_str().to_owned(),
                            size: c.size,
                            ..InFlight::default()
                        };
                        (c.index, f)
                    })
                    .collect(),
            );
            progress.stats(out, &in_flight);
            let call = BulkCall {
                http: self.http.clone(),
                site: s.site.clone(),
                user: s.user.clone(),
                password: s.password.clone(),
                parts: request
                    .iter()
                    .map(|c| PartSource {
                        index: c.index,
                        from: job.source_of(&job.items[c.index]).unwrap_or_default(),
                        path: bulk::file_path(&dest.root, job.items[c.index].dest.as_str()),
                        mtime: c.mtime_ms.div_euclid(1000),
                    })
                    .collect(),
            };
            set.spawn(async move {
                let _permit = permit;
                (n, call.run().await)
            });
        }
        while let Some(done) = set.join_next().await {
            if let Err(e) = self.finished(done, out, progress, &mut in_flight) {
                refusal.get_or_insert(e);
            }
        }
        match refusal {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// One bulk request finished: its files' outcomes, `ItemDone` and stats.
    fn finished(
        &self,
        done: std::result::Result<Finished, tokio::task::JoinError>,
        out: &mut Outcomes,
        progress: &Progress<'_>,
        in_flight: &mut HashMap<usize, Vec<(usize, InFlight)>>,
    ) -> Result<()> {
        let Ok((n, answer)) = done else {
            return Ok(());
        };
        let request = in_flight.remove(&n).unwrap_or_default();
        let answer = match answer {
            Ok(files) => files,
            Err(e) if e.kind == ErrorKind::Auth => return Err(e),
            Err(e) => {
                if e.kind == ErrorKind::Unsupported {
                    self.offers
                        .lock()
                        .expect("offers lock")
                        .insert(progress.base.clone(), false);
                }
                tracing::debug!("bulk upload failed, sending through rclone: {}", e.message);
                out.to_inner.extend(request.iter().map(|(index, _)| index));
                progress.stats(out, in_flight);
                return Ok(());
            }
        };
        for (index, sent) in answer {
            match sent {
                Sent::Stored(size) => {
                    out.done(index, size);
                    progress
                        .sink
                        .send(ProgressEvent::ItemDone { index, ok: true });
                }
                Sent::Missing(e) => {
                    out.outcomes[index] = Some(ItemOutcome::Failed(e));
                    progress
                        .sink
                        .send(ProgressEvent::ItemDone { index, ok: false });
                }
                Sent::NotTaken => out.to_inner.push(index),
            }
        }
        progress.stats(out, in_flight);
        Ok(())
    }

    /// Step 3: what bulk didn't send goes to the inner transport in one call, its progress
    /// renumbered to `job`'s items and added to what bulk sent.
    async fn send_rest(
        &self,
        dest: &Destination,
        job: &TransferJob,
        out: &mut Outcomes,
        progress: &Progress<'_>,
    ) -> Result<()> {
        out.to_inner.sort_unstable();
        out.to_inner.dedup();
        if out.to_inner.is_empty() {
            return Ok(());
        }
        let rest = TransferJob {
            id: job.id.clone(),
            source_roots: job.source_roots.clone(),
            items: out.to_inner.iter().map(|&i| job.items[i].clone()).collect(),
            options: job.options.clone(),
        };
        let (sink, mut rx) = ProgressSink::channel();
        let index_of = out.to_inner.clone();
        let (outer, base) = (progress.sink.clone(), (out.bytes, out.files));
        let totals = progress.totals;
        let forward = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                outer.send(match event {
                    ProgressEvent::ItemDone { index, ok } => ProgressEvent::ItemDone {
                        index: index_of[index],
                        ok,
                    },
                    ProgressEvent::Stats(mut s) => {
                        s.bytes += base.0;
                        s.files_done += base.1;
                        (s.total_files, s.total_bytes) = totals;
                        ProgressEvent::Stats(s)
                    }
                    other => other,
                });
            }
        });
        let answered = self.inner.transfer(dest, &rest, sink).await;
        let _ = forward.await;
        for (&index, outcome) in out.to_inner.clone().iter().zip(answered?.items) {
            if matches!(outcome, ItemOutcome::Done) {
                out.bytes += job.items[index].size;
                out.files += 1;
            }
            out.outcomes[index] = Some(outcome);
        }
        Ok(())
    }
}

/// A Nextcloud to send to by bulk upload, signed in.
struct SignedIn {
    site: Site,
    user: String,
    password: Arc<SecretValue>,
}

fn refused(site: &Site) -> TransportError {
    TransportError::new(
        ErrorKind::Auth,
        format!("{} refused the sign-in (401 Unauthorized)", site.base),
    )
}

/// A file that may go by bulk upload.
struct Candidate {
    index: usize,
    /// Its size now, at the source.
    size: u64,
    mtime_ms: i64,
}

/// Groups `files`, in order, into requests within the files and bytes caps.
fn requests(files: Vec<Candidate>) -> Vec<Vec<Candidate>> {
    let mut out: Vec<Vec<Candidate>> = Vec::new();
    let mut bytes = 0;
    for c in files {
        let fits = out
            .last()
            .is_some_and(|r| r.len() < MAX_REQUEST_FILES && bytes + c.size <= MAX_REQUEST_BYTES);
        if !fits {
            out.push(Vec::new());
            bytes = 0;
        }
        bytes += c.size;
        out.last_mut().expect("a request was just pushed").push(c);
    }
    out
}

enum There<'a> {
    No,
    Exactly(&'a RemoteEntry),
    /// Only in another normalisation or case.
    Loosely,
}

fn there<'a>(entries: &'a [RemoteEntry], name: &str) -> There<'a> {
    if let Some(e) = entries.iter().find(|e| e.name == name) {
        return There::Exactly(e);
    }
    let key = loose_key(name);
    if entries.iter().any(|e| loose_key(&e.name) == key) {
        There::Loosely
    } else {
        There::No
    }
}

/// Whether `e` is `c` already sent: same size, and the same modification time to the second
/// (what rclone compares before skipping a file).
fn same_file(e: &RemoteEntry, c: &Candidate) -> bool {
    !e.is_dir && e.size == Some(c.size) && e.mtime.is_some_and(|m| (m - c.mtime_ms).abs() < 1000)
}

/// The outcomes of a batch as they are decided, and what bulk upload sent.
struct Outcomes {
    outcomes: Vec<Option<ItemOutcome>>,
    to_inner: Vec<usize>,
    bytes: u64,
    files: u64,
}

impl Outcomes {
    fn done(&mut self, index: usize, size: u64) {
        self.outcomes[index] = Some(ItemOutcome::Done);
        self.bytes += size;
        self.files += 1;
    }

    fn skipped(&mut self, index: usize) {
        self.outcomes[index] = Some(ItemOutcome::Done);
        self.files += 1;
    }
}

/// Where the batch's progress goes.
struct Progress<'a> {
    sink: &'a ProgressSink,
    started: Instant,
    /// (files, bytes) of the whole batch.
    totals: (u64, u64),
    base: String,
}

impl Progress<'_> {
    fn stats(&self, out: &Outcomes, in_flight: &HashMap<usize, Vec<(usize, InFlight)>>) {
        let elapsed = self.started.elapsed().as_secs_f64();
        self.sink.send(ProgressEvent::Stats(StatsSnapshot {
            bytes: out.bytes,
            total_bytes: self.totals.1,
            files_done: out.files,
            total_files: self.totals.0,
            bytes_per_sec: if elapsed > 0.0 {
                out.bytes as f64 / elapsed
            } else {
                0.0
            },
            errors: 0,
            elapsed_secs: elapsed,
            transferring: in_flight
                .values()
                .flatten()
                .map(|(_, f)| f.clone())
                .collect(),
        }));
    }
}

/// A bulk request's number and what became of its files.
type Finished = (usize, Result<Vec<(usize, Sent)>>);

/// What became of one file of a bulk request.
enum Sent {
    /// Stored, with its size.
    Stored(u64),
    /// Its source isn't there.
    Missing(TransportError),
    /// Not sent or not stored: the inner transport tries it.
    NotTaken,
}

/// A file to read for a bulk request.
struct PartSource {
    index: usize,
    from: std::path::PathBuf,
    path: String,
    mtime: i64,
}

/// One bulk request, run on its own task.
struct BulkCall {
    http: reqwest::Client,
    site: Site,
    user: String,
    password: Arc<SecretValue>,
    parts: Vec<PartSource>,
}

impl BulkCall {
    /// Reads the files, sends them, and says what became of each. A refused sign-in is `Auth`;
    /// a server without bulk upload `Unsupported`; any other failure of the whole request an
    /// error the caller answers by trying its files through the inner transport.
    async fn run(self) -> Result<Vec<(usize, Sent)>> {
        let mut out = Vec::with_capacity(self.parts.len());
        let mut parts = Vec::with_capacity(self.parts.len());
        let mut sent = Vec::with_capacity(self.parts.len());
        for p in self.parts {
            match tokio::fs::read(&p.from).await {
                Ok(data) => {
                    sent.push((p.index, p.path.clone(), data.len() as u64));
                    parts.push(Part {
                        path: p.path,
                        data,
                        mtime: p.mtime,
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => out.push((
                    p.index,
                    Sent::Missing(TransportError::not_found(format!(
                        "{}: {e}",
                        p.from.display()
                    ))),
                )),
                Err(_) => out.push((p.index, Sent::NotTaken)),
            }
        }
        if parts.is_empty() {
            return Ok(out);
        }
        let boundary = bulk::boundary(&parts)?;
        let body = bulk::body(&parts, &boundary);
        drop(parts);
        let r = self
            .http
            .post(self.site.bulk())
            .basic_auth(&self.user, Some(self.password.expose()))
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/related; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await
            .map_err(|e| TransportError::retryable(format!("bulk upload: {e}")))?;
        match r.status() {
            s if s.is_success() => {}
            StatusCode::UNAUTHORIZED => return Err(refused(&self.site)),
            s @ (StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::NOT_IMPLEMENTED) => {
                return Err(TransportError::unsupported(format!("bulk upload: {s}")));
            }
            s => return Err(TransportError::fatal(format!("bulk upload: {s}"))),
        }
        let answers = r
            .json::<Value>()
            .await
            .map(|v| bulk::answers(&v))
            .map_err(|e| TransportError::fatal(format!("bulk upload: bad answer: {e}")))?;
        for (index, path, size) in sent {
            out.push(match answers.get(&path) {
                Some(None) => (index, Sent::Stored(size)),
                _ => (index, Sent::NotTaken),
            });
        }
        Ok(out)
    }
}

/// The app's bandwidth limit, for bulk requests: each request waits for its turn, so the bytes
/// sent average out to the limit. The inner transport keeps its own.
#[derive(Default)]
struct Limiter {
    rate: Mutex<Option<u64>>,
    next: Mutex<Option<Instant>>,
}

impl Limiter {
    fn set(&self, bytes_per_sec: Option<u64>) {
        *self.rate.lock().expect("limit lock") = bytes_per_sec.filter(|&r| r > 0);
    }

    async fn wait(&self, bytes: u64) {
        let Some(rate) = *self.rate.lock().expect("limit lock") else {
            return;
        };
        let start = {
            let mut next = self.next.lock().expect("limit lock");
            let now = Instant::now();
            let start = next.map_or(now, |n| n.max(now));
            *next = Some(start + Duration::from_secs_f64(bytes as f64 / rate as f64));
            start
        };
        tokio::time::sleep_until(start).await;
    }
}

#[async_trait]
impl Transport for NextcloudBulk {
    fn id(&self) -> &str {
        self.inner.id()
    }

    fn capabilities(&self) -> TransportCaps {
        self.inner.capabilities()
    }

    async fn preflight(&self, dest: &Destination) -> Result<PreflightReport> {
        self.inner.preflight(dest).await
    }

    async fn transfer(
        &self,
        dest: &Destination,
        job: &TransferJob,
        progress: ProgressSink,
    ) -> Result<TransferResult> {
        let Some(s) = self.signed_in(dest).await? else {
            return self.inner.transfer(dest, job, progress).await;
        };
        let mut out = Outcomes {
            outcomes: vec![None; job.items.len()],
            to_inner: Vec::new(),
            bytes: 0,
            files: 0,
        };
        let mut candidates = Vec::new();
        for (index, item) in job.items.iter().enumerate() {
            let small = item.size <= MAX_FILE && !item.dest.as_str().contains(['\r', '\n']);
            let meta = match job.source_of(item) {
                Some(from) if small => tokio::fs::metadata(&from).await.map_err(|e| (from, e)),
                _ => {
                    out.to_inner.push(index);
                    continue;
                }
            };
            match meta {
                Ok(m) if m.is_file() && m.len() <= MAX_FILE => candidates.push(Candidate {
                    index,
                    size: m.len(),
                    mtime_ms: m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_millis() as i64),
                }),
                Err((from, e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    out.outcomes[index] = Some(ItemOutcome::Failed(TransportError::not_found(
                        format!("{}: {e}", from.display()),
                    )));
                }
                _ => out.to_inner.push(index),
            }
        }
        // One small file isn't worth a listing and a request of its own.
        if candidates.len() < 2 {
            return self.inner.transfer(dest, job, progress).await;
        }
        let progress = Progress {
            sink: &progress,
            started: Instant::now(),
            totals: (
                job.items.len() as u64,
                job.items.iter().map(|i| i.size).sum(),
            ),
            base: s.site.base.clone(),
        };
        progress.sink.send(ProgressEvent::Stage("bulk".into()));
        let to_send = self.sort(dest, job, candidates, &s, &mut out).await?;
        for (index, outcome) in out.outcomes.iter().enumerate() {
            if matches!(outcome, Some(ItemOutcome::Done)) {
                progress
                    .sink
                    .send(ProgressEvent::ItemDone { index, ok: true });
            }
        }
        self.send_bulk(dest, job, to_send, &s, &mut out, &progress)
            .await?;
        progress.sink.send(ProgressEvent::Stage("copy".into()));
        self.send_rest(dest, job, &mut out, &progress).await?;

        let items: Vec<ItemOutcome> = out
            .outcomes
            .into_iter()
            .map(|o| {
                o.unwrap_or_else(|| {
                    ItemOutcome::Failed(TransportError::fatal("not sent (bulk upload)"))
                })
            })
            .collect();
        let bytes = job
            .items
            .iter()
            .zip(&items)
            .filter(|(_, o)| matches!(o, ItemOutcome::Done))
            .map(|(i, _)| i.size)
            .sum();
        Ok(TransferResult { items, bytes })
    }

    async fn hash(&self, dest: &Destination, path: &RemotePath, algo: HashAlgo) -> Result<Digest> {
        self.inner.hash(dest, path, algo).await
    }

    async fn hash_many(
        &self,
        dest: &Destination,
        paths: &[RemotePath],
        algo: HashAlgo,
    ) -> Vec<Result<Digest>> {
        self.inner.hash_many(dest, paths, algo).await
    }

    async fn list(
        &self,
        dest: &Destination,
        dir: Option<&RemotePath>,
        dirs_only: bool,
    ) -> Result<Vec<RemoteEntry>> {
        self.inner.list(dest, dir, dirs_only).await
    }

    async fn move_path(&self, dest: &Destination, req: &MoveRequest) -> Result<()> {
        self.inner.move_path(dest, req).await
    }

    async fn move_unchecked(
        &self,
        dest: &Destination,
        from: &RemotePath,
        to: &RemotePath,
        is_dir: bool,
    ) -> Result<()> {
        self.inner.move_unchecked(dest, from, to, is_dir).await
    }

    async fn set_bandwidth_limit(&self, bytes_per_sec: Option<u64>) -> Result<()> {
        self.limit.set(bytes_per_sec);
        self.inner.set_bandwidth_limit(bytes_per_sec).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(index: usize, size: u64) -> Candidate {
        Candidate {
            index,
            size,
            mtime_ms: 0,
        }
    }

    #[test]
    fn requests_keep_order_within_the_caps() {
        let files = (0..250).map(|i| c(i, 10)).collect();
        let sizes: Vec<usize> = requests(files).iter().map(Vec::len).collect();
        assert_eq!(sizes, [100, 100, 50]);
        let big = vec![
            c(0, MAX_FILE),
            c(1, MAX_FILE),
            c(2, MAX_FILE),
            c(3, MAX_FILE),
            c(4, 1),
        ];
        let split: Vec<Vec<usize>> = requests(big)
            .iter()
            .map(|r| r.iter().map(|c| c.index).collect())
            .collect();
        assert_eq!(split, [vec![0, 1, 2, 3], vec![4]]);
    }

    #[test]
    fn a_file_already_there_is_one_of_the_same_size_and_second() {
        let e = |name: &str, size, mtime| RemoteEntry {
            name: name.into(),
            is_dir: false,
            size: Some(size),
            mtime: Some(mtime),
        };
        let file = Candidate {
            index: 0,
            size: 3,
            mtime_ms: 1_700_000_000_400,
        };
        assert!(same_file(&e("a", 3, 1_700_000_000_000), &file));
        assert!(!same_file(&e("a", 4, 1_700_000_000_000), &file));
        assert!(!same_file(&e("a", 3, 1_700_000_002_000), &file));
        let listed = [e("Ver\u{e3}o.jpg", 1, 0), e("x.txt", 1, 0)];
        assert!(matches!(there(&listed, "x.txt"), There::Exactly(_)));
        assert!(matches!(there(&listed, "VERA\u{303}O.JPG"), There::Loosely));
        assert!(matches!(there(&listed, "y.txt"), There::No));
    }

    #[tokio::test]
    async fn the_limit_spaces_requests_out() {
        let l = Limiter::default();
        l.set(Some(1000));
        let t = Instant::now();
        l.wait(100).await;
        l.wait(100).await;
        l.wait(100).await;
        // Three requests of 100 bytes at 1000 B/s: the third starts after 0.2 s.
        assert!(
            t.elapsed() >= Duration::from_millis(190),
            "{:?}",
            t.elapsed()
        );
        l.set(None);
        let t = Instant::now();
        l.wait(1 << 30).await;
        assert!(t.elapsed() < Duration::from_millis(50));
    }
}
