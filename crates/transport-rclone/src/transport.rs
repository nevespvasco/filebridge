use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use filebridge_names::loose_key;
use filebridge_transport_api::{
    Backend, Check, CheckStatus, Destination, Digest, ErrorKind, HashAlgo, ItemOutcome, KeyringRef,
    PreflightReport, ProgressEvent, ProgressSink, RemoteEntry, RemotePath, Result, SecretResolver,
    SecretValue, TransferJob, TransferResult, Transport, TransportCaps, TransportError,
};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::fs::{Creds, dest_fs, local_fs, utf8};
use crate::{RcClient, stats};

pub struct RcloneTransport {
    rc: RcClient,
    /// Reads profiles' passwords and keys (the host's, over the OS keyring); without one,
    /// profiles that need a secret fail.
    secrets: Option<Arc<dyn SecretResolver>>,
}

impl RcloneTransport {
    pub const ID: &'static str = "rclone";

    /// A transport over `rc`: an `Arc<Rcd>` (desktop) or an `Arc<Librclone>` (phones).
    pub fn new(rc: impl Into<RcClient>) -> Self {
        Self {
            rc: rc.into(),
            secrets: None,
        }
    }

    /// Reads profiles' secrets through `resolver`.
    pub fn with_secrets(mut self, resolver: Arc<dyn SecretResolver>) -> Self {
        self.secrets = Some(resolver);
        self
    }

    /// The destination's secrets for one call, obscured by rclone, which only takes passwords
    /// that way. Each value (clear and as rclone takes it) is scrubbed from every rclone message
    /// while the `Creds` live.
    async fn creds(&self, dest: &Destination) -> Result<Creds> {
        let (password, key) = match &dest.backend {
            Backend::Sftp { password, key, .. } => (password.clone(), key.clone()),
            Backend::Webdav { password, .. } => (password.clone(), None),
            Backend::Local => (None, None),
        };
        let scrubber = self.rc().scrubber();
        let mut creds = Creds::default();
        if let Some(r) = password {
            let clear = self.secret(&r).await?;
            creds.held.push(scrubber.hold(clear.expose()));
            let obscured = self
                .rc()
                .call("core/obscure", json!({ "clear": clear.expose() }))
                .await?;
            let obscured = obscured
                .get("obscured")
                .and_then(Value::as_str)
                .ok_or_else(|| TransportError::fatal("rclone rc core/obscure: no answer"))?;
            creds.held.push(scrubber.hold(obscured));
            creds.pass = Some(SecretValue::new(obscured.to_owned()));
        }
        if let Some(r) = key {
            // rclone wants the PEM on one line, its line breaks written as `\n`.
            let pem = self.secret(&r).await?;
            let one_line = pem.expose().trim_end().replace('\n', "\\n");
            creds.held.push(scrubber.hold(pem.expose()));
            creds.held.push(scrubber.hold(&one_line));
            creds.key_pem = Some(SecretValue::new(one_line));
        }
        Ok(creds)
    }

    /// One secret, through the host's resolver: missing is `Auth`, an unreachable keyring
    /// `Transient`.
    async fn secret(&self, r: &KeyringRef) -> Result<SecretValue> {
        let resolver = self.secrets.as_ref().ok_or_else(|| {
            TransportError::fatal("no keyring available to read the destination's credentials")
        })?;
        resolver.resolve(r).await
    }

    fn rc(&self) -> &RcClient {
        &self.rc
    }

    /// [`Transport::hash_many`]'s calls, one folder at a time: first the server's own hashes,
    /// then downloads for the files it didn't hash. Fills in `out` what they answer; a
    /// `Transient` error answers for the folder's files, other errors leave them to `hash`.
    async fn hash_by_folder(
        &self,
        dest: &Destination,
        creds: &Creds,
        paths: &[RemotePath],
        algo: HashAlgo,
        out: &mut [Option<Result<Digest>>],
    ) {
        let Ok(hash_type) = hash_type(algo) else {
            return;
        };
        let mut folders: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (i, path) in paths.iter().enumerate() {
            folders
                .entry(path.parent().unwrap_or(""))
                .or_default()
                .push(i);
        }
        // Once the server hashed none of a folder's files, it can't: the other folders go
        // straight to downloading.
        let mut server_hashes = true;
        for (folder, mut left) in folders {
            let Ok(fs) = dest_fs(dest, creds, folder) else {
                continue;
            };
            for download in [false, true] {
                if left.is_empty() || (!download && !server_hashes) {
                    continue;
                }
                let rules: Vec<String> = left
                    .iter()
                    .map(|&i| format!("/{}", glob_escape(paths[i].file_name())))
                    .collect();
                let params = json!({
                    "fs": fs, "hashType": hash_type, "download": download,
                    "_filter": { "IncludeRule": rules },
                    "_config": { "MaxDepth": 1, "Transfers": HASH_DOWNLOADS },
                });
                let found: HashMap<String, Vec<u8>> =
                    match self.rc().call("operations/hashsum", params).await {
                        Ok(v) => hashsum_lines(&v)
                            .filter_map(|l| parse_hashsum_line(l, algo))
                            .map(|(bytes, name)| (name.to_owned(), bytes))
                            .collect(),
                        Err(e) if e.is_retryable() => {
                            for i in left.drain(..) {
                                out[i] = Some(Err(e.clone()));
                            }
                            continue;
                        }
                        Err(_) => HashMap::new(),
                    };
                if !download && found.is_empty() {
                    server_hashes = false;
                }
                left.retain(|&i| match found.get(paths[i].file_name()) {
                    Some(bytes) => {
                        out[i] = Some(Ok(Digest {
                            algo,
                            bytes: bytes.clone(),
                        }));
                        false
                    }
                    None => true,
                });
            }
        }
    }

    /// What is already in each destination folder that two or more of `job`'s items go to, as
    /// [`loose_key`]s: one `operations/list` per folder, `concurrency` at a time. A folder that
    /// isn't there is empty; one that can't be listed is left out, so its files keep rclone's
    /// own check. An `Auth` error fails the call before anything is sent.
    async fn listed_folders(
        &self,
        dst_fs: &str,
        job: &TransferJob,
    ) -> Result<HashMap<String, HashSet<String>>> {
        let mut count: HashMap<&str, usize> = HashMap::new();
        for item in &job.items {
            *count.entry(item.dest.parent().unwrap_or("")).or_default() += 1;
        }
        let sem = Arc::new(Semaphore::new(job.options.concurrency.max(1)));
        let mut set = JoinSet::new();
        for (folder, n) in count {
            if n < 2 {
                continue;
            }
            let permit = sem
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            let rc = self.rc().clone();
            let (folder, params) = (folder.to_owned(), json!({ "fs": dst_fs, "remote": folder }));
            set.spawn(async move {
                let _permit = permit;
                (folder, rc.call("operations/list", params).await)
            });
        }
        let mut listed = HashMap::new();
        while let Some(done) = set.join_next().await {
            let Ok((folder, answer)) = done else { continue };
            let names = match answer {
                Ok(v) => remote_entries(&v).map(|e| loose_key(&e.name)).collect(),
                Err(e) if e.kind == ErrorKind::NotFound => HashSet::new(),
                Err(e) if e.kind == ErrorKind::Auth => return Err(e),
                Err(_) => continue,
            };
            listed.insert(folder, names);
        }
        Ok(listed)
    }

    /// One `operations/copyfile` per item, `concurrency` at a time. A file whose folder was
    /// listed ([`Self::listed_folders`]) and holds nothing that answers to its name (exactly, in
    /// another normalisation or in another case) is sent without rclone's existence check: one
    /// request fewer per file, a quarter of them for small files on Nextcloud. Every other file
    /// keeps the check, so one already there and identical is skipped (resume).
    ///
    /// The first `Auth` error ends the round: nothing more is started (each start would be one
    /// more refused sign-in, which servers count against the account), the files already on
    /// their way finish, and the call fails with that error. Files that arrived were already
    /// reported by `ItemDone`.
    ///
    /// The first `Full` error stops the round the same way, without failing the call: every
    /// file not started yet fails with that error too, instead of each being uploaded in turn
    /// to a destination with no room.
    async fn per_file(
        &self,
        dst_fs: &str,
        group: &str,
        job: &TransferJob,
        progress: &ProgressSink,
    ) -> Result<Vec<ItemOutcome>> {
        let listed = self.listed_folders(dst_fs, job).await?;
        let src_fs: Vec<Result<String>> = job.source_roots.iter().map(|r| local_fs(r)).collect();
        let mut outcomes = vec![ItemOutcome::Done; job.items.len()];
        let mut refused: Option<TransportError> = None;
        let mut full: Option<TransportError> = None;
        let mut stopped_at = None;
        let sem = Arc::new(Semaphore::new(job.options.concurrency.max(1)));
        let mut set = JoinSet::new();
        for (i, item) in job.items.iter().enumerate() {
            let root = src_fs.get(item.root).cloned().unwrap_or_else(|| {
                Err(TransportError::fatal(format!(
                    "{}: unknown source root {}",
                    item.dest, item.root
                )))
            });
            let (src_fs, src) = match root.and_then(|fs| Ok((fs, utf8(&item.source)?.to_owned()))) {
                Ok(s) => s,
                Err(e) => {
                    outcomes[i] = ItemOutcome::Failed(e);
                    continue;
                }
            };
            let permit = sem
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            while let Some(done) = set.try_join_next() {
                record(done, &mut outcomes, progress, &mut refused, &mut full);
            }
            if refused.is_some() || full.is_some() {
                stopped_at = Some(i);
                break;
            }
            let rc = self.rc().clone();
            let mut params = json!({
                "srcFs": src_fs, "srcRemote": src,
                "dstFs": dst_fs, "dstRemote": item.dest.as_str(),
                "_group": group,
            });
            if is_absent(&listed, &item.dest) {
                params["_config"] = json!({ "NoCheckDest": true });
            }
            set.spawn(async move {
                let _permit = permit;
                (i, rc.call("operations/copyfile", params).await)
            });
        }
        while let Some(done) = set.join_next().await {
            record(done, &mut outcomes, progress, &mut refused, &mut full);
        }
        if let (Some(e), Some(at)) = (&full, stopped_at) {
            for outcome in &mut outcomes[at..] {
                *outcome = ItemOutcome::Failed(e.clone());
            }
        }
        match refused {
            Some(e) => Err(e),
            None => Ok(outcomes),
        }
    }
}

/// One finished copy: its outcome, its `ItemDone`, the first `Auth` error in `refused` and the
/// first `Full` one in `full`.
fn record(
    done: std::result::Result<(usize, Result<Value>), tokio::task::JoinError>,
    outcomes: &mut [ItemOutcome],
    progress: &ProgressSink,
    refused: &mut Option<TransportError>,
    full: &mut Option<TransportError>,
) {
    let Ok((i, res)) = done else { return };
    let ok = res.is_ok();
    if let Err(e) = res {
        if e.kind == ErrorKind::Auth {
            refused.get_or_insert_with(|| e.clone());
        } else if e.kind == ErrorKind::Full {
            full.get_or_insert_with(|| e.clone());
        }
        outcomes[i] = ItemOutcome::Failed(e);
    }
    progress.send(ProgressEvent::ItemDone { index: i, ok });
}

#[async_trait]
impl Transport for RcloneTransport {
    fn id(&self) -> &str {
        Self::ID
    }

    fn capabilities(&self) -> TransportCaps {
        // rclone skips files already identical at the destination (resume), uploads to a
        // `.partial` name and renames on completion, and computes SHA-256 over SFTP.
        TransportCaps::RESUME
            | TransportCaps::HASH_SHA256
            | TransportCaps::RENAME
            | TransportCaps::PARTIAL_HIDDEN
            | TransportCaps::LIST
            | TransportCaps::MOVE
    }

    async fn preflight(&self, dest: &Destination) -> Result<PreflightReport> {
        let creds = self.creds(dest).await?;
        let started = Instant::now();
        let about = self
            .rc()
            .call(
                "operations/about",
                json!({ "fs": dest_fs(dest, &creds, "")? }),
            )
            .await;
        let latency_ms = Some(started.elapsed().as_millis() as u64);
        let mut report = PreflightReport {
            latency_ms,
            ..Default::default()
        };
        match about {
            Ok(v) => {
                report.free_bytes = v.get("free").and_then(Value::as_u64);
                report.total_bytes = v.get("total").and_then(Value::as_u64);
                report.used_bytes = v.get("used").and_then(Value::as_u64);
                report.checks.push(Check {
                    id: "connect".into(),
                    status: CheckStatus::Ok,
                    message: "destination reachable".into(),
                });
            }
            Err(e) => report.checks.push(Check {
                id: "connect".into(),
                status: CheckStatus::Fail,
                message: e.message,
            }),
        }
        report.checks.push(Check {
            id: "writable".into(),
            status: CheckStatus::Warn,
            message: "write permission is not probed yet (Phase 1)".into(),
        });
        Ok(report)
    }

    async fn move_unchecked(
        &self,
        dest: &Destination,
        from: &RemotePath,
        to: &RemotePath,
        is_dir: bool,
    ) -> Result<()> {
        let creds = self.creds(dest).await?;
        if is_dir {
            // `move_path` made sure nothing is at `to`, so rclone does one server-side DirMove.
            self.rc()
                .call_job(
                    "sync/move",
                    json!({
                        "srcFs": dest_fs(dest, &creds, from.as_str())?,
                        "dstFs": dest_fs(dest, &creds, to.as_str())?,
                        "deleteEmptySrcDirs": true,
                    }),
                )
                .await?;
        } else {
            let root = dest_fs(dest, &creds, "")?;
            self.rc()
                .call(
                    "operations/movefile",
                    json!({
                        "srcFs": root, "srcRemote": from.as_str(),
                        "dstFs": root, "dstRemote": to.as_str(),
                    }),
                )
                .await?;
        }
        Ok(())
    }

    async fn list(
        &self,
        dest: &Destination,
        dir: Option<&RemotePath>,
        dirs_only: bool,
    ) -> Result<Vec<RemoteEntry>> {
        let creds = self.creds(dest).await?;
        let v = self
            .rc()
            .call(
                "operations/list",
                json!({
                    "fs": dest_fs(dest, &creds, "")?,
                    "remote": dir.map_or("", RemotePath::as_str),
                    "opt": { "dirsOnly": dirs_only },
                }),
            )
            .await?;
        let mut out: Vec<RemoteEntry> = remote_entries(&v)
            .filter(|e| !dirs_only || e.is_dir)
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn transfer(
        &self,
        dest: &Destination,
        job: &TransferJob,
        progress: ProgressSink,
    ) -> Result<TransferResult> {
        let creds = self.creds(dest).await?;
        let rc = self.rc();
        let group = format!("job-{}", job.id);
        let dst_fs = dest_fs(dest, &creds, "")?;
        let totals = (
            job.items.len() as u64,
            job.items.iter().map(|i| i.size).sum::<u64>(),
        );
        let names = Arc::new(stats::InFlightNames::of(job));
        let poller = stats::spawn_poller(
            rc.clone(),
            group.clone(),
            Some(totals),
            Some(names.clone()),
            progress.clone(),
            Duration::from_millis(500),
        );

        progress.send(ProgressEvent::Stage("copy".into()));
        let outcomes = self.per_file(&dst_fs, &group, job, &progress).await;
        poller.abort();
        if let Ok(s) = stats::snapshot(rc, &group, Some(&names)).await {
            progress.send(ProgressEvent::Stats(s));
        }
        // Each call has its own group, so rclone would keep every one of them until it exits.
        let _ = rc
            .call("core/stats-delete", json!({ "group": group }))
            .await;
        let outcomes = outcomes?;
        let bytes = job
            .items
            .iter()
            .zip(&outcomes)
            .filter(|(_, o)| matches!(o, ItemOutcome::Done))
            .map(|(i, _)| i.size)
            .sum();
        Ok(TransferResult {
            items: outcomes,
            bytes,
        })
    }

    async fn hash(&self, dest: &Destination, path: &RemotePath, algo: HashAlgo) -> Result<Digest> {
        let creds = self.creds(dest).await?;
        let hash_type = hash_type(algo)?;
        let fs = dest_fs(dest, &creds, path.as_str())?;
        let params =
            |download: bool| json!({ "fs": fs, "hashType": hash_type, "download": download });
        let first = |v: &Value| {
            hashsum_lines(v)
                .next()
                .and_then(|l| parse_hashsum_line(l, algo))
                .map(|(bytes, _)| bytes)
        };
        // Servers that can't hash (no `sha256sum`, or `rclone serve sftp`) fall back to rclone
        // downloading the file and hashing it locally: slower, but still a real check. rclone
        // reports "hash unsupported" as an empty list (only logged), or as an error.
        let bytes = match self.rc().call("operations/hashsum", params(false)).await {
            Ok(v) => first(&v),
            Err(e) if e.kind == ErrorKind::Unsupported => None,
            Err(e) => return Err(e),
        };
        let bytes = match bytes {
            Some(b) => b,
            None => {
                let v = self.rc().call("operations/hashsum", params(true)).await?;
                first(&v)
                    .ok_or_else(|| TransportError::fatal(format!("no hash returned for {path}")))?
            }
        };
        Ok(Digest { algo, bytes })
    }

    /// One `operations/hashsum` per folder, limited to the asked names by include rules: one
    /// listing for the folder instead of one per file (rclone lists a file's whole folder to
    /// find it), and the downloads of servers that can't hash run `HASH_DOWNLOADS` at a time.
    /// What the folder's call doesn't answer (a file that isn't there, an error) is asked of
    /// [`Transport::hash`], one by one, so each answer is the one `hash` gives.
    async fn hash_many(
        &self,
        dest: &Destination,
        paths: &[RemotePath],
        algo: HashAlgo,
    ) -> Vec<Result<Digest>> {
        let mut out: Vec<Option<Result<Digest>>> = vec![None; paths.len()];
        if hash_type(algo).is_ok() && !paths.is_empty() {
            match self.creds(dest).await {
                Ok(creds) => {
                    self.hash_by_folder(dest, &creds, paths, algo, &mut out)
                        .await
                }
                Err(e) => out.fill(Some(Err(e))),
            }
        }
        let mut answers = Vec::with_capacity(paths.len());
        for (path, answer) in paths.iter().zip(out) {
            answers.push(match answer {
                Some(a) => a,
                None => self.hash(dest, path, algo).await,
            });
        }
        answers
    }

    async fn set_bandwidth_limit(&self, bytes_per_sec: Option<u64>) -> Result<()> {
        // `core/bwlimit` is global to the rcd: it applies to every transfer, running ones too.
        self.rc()
            .call(
                "core/bwlimit",
                json!({ "rate": bwlimit_rate(bytes_per_sec) }),
            )
            .await?;
        Ok(())
    }
}

/// The entries of an `operations/list` answer.
fn remote_entries(v: &Value) -> impl Iterator<Item = RemoteEntry> + '_ {
    v.get("list")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some(RemoteEntry {
                name: e.get("Name")?.as_str()?.to_owned(),
                is_dir: e.get("IsDir").and_then(Value::as_bool).unwrap_or(false),
                size: e
                    .get("Size")
                    .and_then(Value::as_i64)
                    .and_then(|s| u64::try_from(s).ok()),
                mtime: e
                    .get("ModTime")
                    .and_then(Value::as_str)
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|t| t.timestamp_millis()),
            })
        })
}

/// Whether `dest`'s folder was listed and holds nothing that answers to its name.
fn is_absent(listed: &HashMap<String, HashSet<String>>, dest: &RemotePath) -> bool {
    listed
        .get(dest.parent().unwrap_or(""))
        .is_some_and(|names| !names.contains(&loose_key(dest.file_name())))
}

/// Files `Transport::hash_many` downloads at once from a server that can't hash them itself.
const HASH_DOWNLOADS: usize = 8;

/// rclone's name for a hash kind.
fn hash_type(algo: HashAlgo) -> Result<&'static str> {
    match algo {
        HashAlgo::Sha256 => Ok("sha256"),
        HashAlgo::Xxh3_128 => Err(TransportError::unsupported(
            "xxh3-128 is not supported by rclone",
        )),
    }
}

/// The lines of an `operations/hashsum` answer.
fn hashsum_lines(v: &Value) -> impl Iterator<Item = &str> {
    v.get("hashsum")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// A `hashsum` line, `<hex>  <path>`, as the hash and the path. `None` for anything else (an
/// empty line, `UNSUPPORTED`, `ERROR`, a hash of the wrong length).
fn parse_hashsum_line(line: &str, algo: HashAlgo) -> Option<(Vec<u8>, &str)> {
    let width = match algo {
        HashAlgo::Sha256 => 64,
        HashAlgo::Xxh3_128 => 32,
    };
    let (hex, path) = line.split_once("  ")?;
    if hex.len() != width {
        return None;
    }
    let bytes = (0..width)
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<_>>>()?;
    Some((bytes, path))
}

/// `name` as an rclone filter glob that matches only itself.
fn glob_escape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        // rclone takes `\` before ASCII only.
        if matches!(c, '\\' | '*' | '?' | '[' | ']' | '{' | '}') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// rclone's `--bwlimit` form: a plain number is KiB/s, so bytes need the `B` suffix.
fn bwlimit_rate(bytes_per_sec: Option<u64>) -> String {
    match bytes_per_sec {
        Some(n) => format!("{n}B"),
        None => "off".into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap, HashSet};
    use std::sync::Arc;

    use super::{RcloneTransport, bwlimit_rate, glob_escape, is_absent, parse_hashsum_line};
    use crate::recording::Recording;
    use filebridge_names::loose_key;
    use filebridge_transport_api::{
        Backend, Destination, ErrorKind, HashAlgo, ItemOutcome, ProgressSink, RemotePath,
        TransferItem, TransferJob, TransferOptions, Transport, TransportError,
    };
    use serde_json::{Value, json};

    fn dest() -> Destination {
        Destination {
            transport: RcloneTransport::ID.into(),
            backend: Backend::Local,
            root: "/dest".into(),
        }
    }

    fn job(paths: &[&str]) -> TransferJob {
        TransferJob {
            id: "j".into(),
            source_roots: vec!["/src".into()],
            items: paths
                .iter()
                .map(|p| TransferItem {
                    root: 0,
                    source: p.into(),
                    dest: RemotePath::new(*p).unwrap(),
                    size: 1,
                    expected_hash: None,
                })
                .collect(),
            options: TransferOptions { concurrency: 2 },
        }
    }

    fn strings(calls: &[Value], key: &str) -> BTreeSet<String> {
        calls
            .iter()
            .map(|p| p[key].as_str().unwrap().to_owned())
            .collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[tokio::test]
    async fn a_send_lists_each_shared_folder_once_and_checks_only_what_is_there() {
        let rc = Arc::new(Recording::new(|method, p| match method {
            // `a` holds x.txt, under another case; the root isn't there yet.
            "operations/list" if p["remote"] == "a" => Some(Ok(
                json!({ "list": [{ "Name": "X.TXT", "IsDir": false, "Size": 1 }] }),
            )),
            "operations/list" => Some(Err(TransportError::not_found("directory not found"))),
            _ => None,
        }));
        let t = RcloneTransport::new(rc.clone());
        let paths = [
            "a/x.txt",
            "a/y.txt",
            "a/z.txt",
            "b/only.txt",
            "r1.txt",
            "r2.txt",
        ];
        let sent = t
            .transfer(&dest(), &job(&paths), ProgressSink::noop())
            .await
            .unwrap();
        assert_eq!(sent.failed(), 0, "{:?}", sent.items);

        // One listing per folder with two or more files; `b` has one, so its file is checked.
        let lists = rc.calls_to("operations/list");
        assert_eq!(strings(&lists, "remote"), set(&["", "a"]));
        assert_eq!(lists.len(), 2);
        let copies = rc.calls_to("operations/copyfile");
        assert_eq!(strings(&copies, "dstRemote"), set(&paths));
        let unchecked: Vec<_> = copies
            .iter()
            .filter(|p| p["_config"]["NoCheckDest"] == true)
            .cloned()
            .collect();
        assert_eq!(
            strings(&unchecked, "dstRemote"),
            set(&["a/y.txt", "a/z.txt", "r1.txt", "r2.txt"])
        );
    }

    #[tokio::test]
    async fn a_refused_listing_sends_nothing() {
        let rc = Arc::new(Recording::new(|method, _| {
            (method == "operations/list")
                .then(|| Err(TransportError::new(ErrorKind::Auth, "401 Unauthorized")))
        }));
        let t = RcloneTransport::new(rc.clone());
        let err = t
            .transfer(&dest(), &job(&["a/1", "a/2"]), ProgressSink::noop())
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Auth);
        assert!(rc.calls_to("operations/copyfile").is_empty());
    }

    #[tokio::test]
    async fn a_full_destination_stops_the_round() {
        let rc = Arc::new(Recording::new(|method, _| match method {
            "operations/list" => Some(Err(TransportError::not_found("directory not found"))),
            "operations/copyfile" => Some(Err(TransportError::full(
                "Failed to copy: no space left on device",
            ))),
            _ => None,
        }));
        let t = RcloneTransport::new(rc.clone());
        let paths: Vec<String> = (0..20).map(|i| format!("f{i}.bin")).collect();
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let sent = t
            .transfer(&dest(), &job(&paths), ProgressSink::noop())
            .await
            .unwrap();
        assert!(
            sent.items
                .iter()
                .all(|o| matches!(o, ItemOutcome::Failed(e) if e.kind == ErrorKind::Full)),
            "{:?}",
            sent.items
        );
        let copies = rc.calls_to("operations/copyfile").len();
        assert!(
            copies < paths.len(),
            "{copies} uploads to a full destination"
        );
    }

    #[tokio::test]
    async fn hashing_many_asks_once_per_folder_and_downloads_only_what_the_server_missed() {
        let hex = |c: char| c.to_string().repeat(64);
        let rc = Arc::new(Recording::new(move |method, p| {
            if method != "operations/hashsum" {
                return None;
            }
            let fs = p["fs"].as_str().unwrap();
            let download = p["download"] == true;
            let lines: Vec<String> = match (fs, download) {
                (":local:/dest/a", false) => {
                    vec![format!("{}  1", hex('a')), format!("{}  2", hex('b'))]
                }
                // The server hashes nothing in `b`: the next pass downloads.
                (":local:/dest/b", false) => vec![],
                (":local:/dest/b", true) => vec![format!("{}  3", hex('c'))],
                _ => return Some(Err(TransportError::not_found("object not found"))),
            };
            Some(Ok(json!({ "hashsum": lines })))
        }));
        let t = RcloneTransport::new(rc.clone());
        let paths: Vec<_> = ["a/1", "a/2", "b/3", "b/gone"]
            .iter()
            .map(|p| RemotePath::new(*p).unwrap())
            .collect();
        let got = t.hash_many(&dest(), &paths, HashAlgo::Sha256).await;
        assert_eq!(got[0].as_ref().unwrap().bytes, vec![0xaa; 32]);
        assert_eq!(got[2].as_ref().unwrap().bytes, vec![0xcc; 32]);
        assert_eq!(got[3].as_ref().unwrap_err().kind, ErrorKind::NotFound);
        // a; b, then b by download; then `hash` for the one path nobody answered.
        let calls = rc.calls_to("operations/hashsum");
        assert_eq!(calls.len(), 4, "{calls:#?}");
        assert_eq!(calls[3]["fs"], ":local:/dest/b/gone");
    }

    #[test]
    fn rate_strings() {
        assert_eq!(bwlimit_rate(Some(2_500_000)), "2500000B");
        assert_eq!(bwlimit_rate(None), "off");
    }

    #[test]
    fn hashsum_lines_and_globs() {
        let hex = "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb";
        let line = format!("{hex}  a  b.txt");
        let (bytes, name) = parse_hashsum_line(&line, HashAlgo::Sha256).unwrap();
        assert_eq!((bytes.len(), bytes[0], name), (32, 0xca, "a  b.txt"));
        for bad in ["", "UNSUPPORTED  a.txt", "ERROR  a.txt", "ca97  a.txt"] {
            assert!(parse_hashsum_line(bad, HashAlgo::Sha256).is_none(), "{bad}");
        }
        let not_hex = format!("{}zz  a.txt", &hex[..62]);
        assert!(parse_hashsum_line(&not_hex, HashAlgo::Sha256).is_none());
        assert_eq!(
            glob_escape("Ver\u{e3}o [1]{a,b}*?\\.txt"),
            "Ver\u{e3}o \\[1\\]\\{a,b\\}\\*\\?\\\\.txt"
        );
    }

    #[test]
    fn only_a_listed_folder_without_the_name_skips_the_check() {
        let listed = HashMap::from([
            (String::new(), HashSet::from([loose_key("top.txt")])),
            (
                "Fotos".to_owned(),
                HashSet::from([loose_key("Ver\u{e3}o.JPG")]),
            ),
        ]);
        let absent = |p: &str| is_absent(&listed, &RemotePath::new(p).unwrap());
        assert!(absent("new.txt"));
        assert!(absent("Fotos/praia.jpg"));
        assert!(!absent("top.txt"), "there exactly");
        assert!(
            !absent("Fotos/Vera\u{303}o.jpg"),
            "there in NFD and another case"
        );
        assert!(!absent("other/new.txt"), "folder not listed");
    }
}
