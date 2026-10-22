use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use filebridge_transport_api::{
    Backend, Check, CheckStatus, Destination, Digest, ErrorKind, HashAlgo, ItemOutcome, KeyringRef,
    PreflightReport, ProgressEvent, ProgressSink, RemoteEntry, RemotePath, Result, SecretResolver,
    SecretValue, TransferJob, TransferResult, Transport, TransportCaps, TransportError,
};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::fs::{Creds, dest_fs, join, local_fs, split_root, utf8};
use crate::{RcClient, Rcd, rename, stats};

/// How files reach their corrected names. See the crate docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    PerFile,
    TempThenRename,
}

pub struct RcloneTransport {
    rcd: Arc<Rcd>,
    strategy: Strategy,
    /// Reads profiles' passwords and keys (the host's, over the OS keyring); without one,
    /// profiles that need a secret fail.
    secrets: Option<Arc<dyn SecretResolver>>,
}

impl RcloneTransport {
    pub const ID: &'static str = "rclone";

    pub fn new(rcd: Arc<Rcd>, strategy: Strategy) -> Self {
        Self {
            rcd,
            strategy,
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
        self.rcd.client()
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

    /// One `operations/copyfile` per item, `concurrency` at a time. The first `Auth` error ends
    /// the round: nothing more is started (each start would be one more refused sign-in, which
    /// servers count against the account), the files already on their way finish, and the call
    /// fails with that error. Files that arrived were already reported by `ItemDone`.
    async fn per_file(
        &self,
        src_fs: &str,
        dst_fs: &str,
        group: &str,
        job: &TransferJob,
        progress: &ProgressSink,
    ) -> Result<Vec<ItemOutcome>> {
        let mut outcomes = vec![ItemOutcome::Done; job.items.len()];
        let mut refused: Option<TransportError> = None;
        let sem = Arc::new(Semaphore::new(job.options.concurrency.max(1)));
        let mut set = JoinSet::new();
        for (i, item) in job.items.iter().enumerate() {
            let src = match utf8(&item.source) {
                Ok(s) => s.to_owned(),
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
                record(done, &mut outcomes, progress, &mut refused);
            }
            if refused.is_some() {
                break;
            }
            let rc = self.rc().clone();
            let params = json!({
                "srcFs": src_fs, "srcRemote": src,
                "dstFs": dst_fs, "dstRemote": item.dest.as_str(),
                "_group": group,
            });
            set.spawn(async move {
                let _permit = permit;
                (i, rc.call("operations/copyfile", params).await)
            });
        }
        while let Some(done) = set.join_next().await {
            record(done, &mut outcomes, progress, &mut refused);
        }
        match refused {
            Some(e) => Err(e),
            None => Ok(outcomes),
        }
    }

    async fn temp_then_rename(
        &self,
        src_fs: &str,
        dest: &Destination,
        group: &str,
        job: &TransferJob,
        progress: &ProgressSink,
    ) -> Result<()> {
        let creds = self.creds(dest).await?;
        let rc = self.rc();
        let concurrency = job.options.concurrency.max(1);
        let (parent, base) = split_root(&dest.root)?;
        let tmp_name = format!(".{base}.filebridge-tmp-{}", job.id);
        let tmp = Destination {
            root: join(parent, &tmp_name),
            ..dest.clone()
        };
        let tmp_fs = dest_fs(&tmp, &creds, "")?;
        let config = json!({ "Transfers": concurrency, "Checkers": concurrency * 2 });

        let mut pairs = Vec::with_capacity(job.items.len());
        for item in &job.items {
            pairs.push((utf8(&item.source)?, item.dest.as_str()));
        }
        let plan = rename::plan(pairs.iter().copied())?;

        // 1. Copy with original names. The file list keeps rclone to exactly the job's items.
        progress.send(ProgressEvent::Stage("copy".into()));
        let list = std::env::temp_dir().join(format!("filebridge-{}-files.txt", job.id));
        let mut body = String::new();
        for (src, _) in &pairs {
            if src.contains('\n') {
                return Err(TransportError::fatal(format!("newline in name: {src:?}")));
            }
            body.push_str(src);
            body.push('\n');
        }
        std::fs::write(&list, body)
            .map_err(|e| TransportError::fatal(format!("write {}: {e}", list.display())))?;
        let copied = rc
            .call_job(
                "sync/copy",
                json!({
                    "srcFs": src_fs, "dstFs": tmp_fs,
                    "_filter": { "FilesFromRaw": [utf8(&list)?] },
                    "_config": config, "_group": group,
                }),
            )
            .await;
        let _ = std::fs::remove_file(&list);
        copied?;

        // 2. Batch rename inside the temp folder: files, then directories deepest-first. Each
        //    rename goes through a unique intermediate name, so it never targets a name the
        //    server may treat as equal to its own source (NFD vs NFC on a normalising
        //    filesystem): there `sync/move` sees "unchanged" files and deletes them.
        progress.send(ProgressEvent::Stage(format!(
            "rename ({} nodes)",
            plan.len()
        )));
        let mid = |from: &str, n: usize| {
            let parent = from.rsplit_once('/').map_or("", |(p, _)| p);
            join(parent, &format!(".fb-{n}-{}", job.id))
        };
        let file_move = |from: &str, to: &str| {
            (
                "operations/movefile",
                json!({
                    "srcFs": tmp_fs, "srcRemote": from,
                    "dstFs": tmp_fs, "dstRemote": to, "_group": group,
                }),
            )
        };
        let mids: Vec<String> = (0..plan.files.len())
            .map(|n| mid(&plan.files[n].0, n))
            .collect();
        let step1 = plan.files.iter().zip(&mids);
        run_all(
            rc,
            step1.map(|((from, _), m)| file_move(from, m)).collect(),
            concurrency,
        )
        .await?;
        let step2 = plan.files.iter().zip(&mids);
        run_all(
            rc,
            step2.map(|((_, to), m)| file_move(m, to)).collect(),
            concurrency,
        )
        .await?;
        for renames in plan.dirs_by_depth.values().rev() {
            let mids: Vec<String> = renames
                .iter()
                .enumerate()
                .map(|(n, (from, _))| mid(from, n))
                .collect();
            for step in 0..2 {
                let mut moves = Vec::with_capacity(renames.len());
                for ((from, to), m) in renames.iter().zip(&mids) {
                    let (a, b) = if step == 0 { (from, m) } else { (m, to) };
                    // The target never exists, so rclone does a single server-side DirMove.
                    moves.push((
                        "sync/move",
                        json!({
                            "srcFs": dest_fs(&tmp, &creds, a)?, "dstFs": dest_fs(&tmp, &creds, b)?,
                            "deleteEmptySrcDirs": true, "_group": group,
                        }),
                    ));
                }
                // One at a time: directory renames are few, and rclone's own VFS server crashes
                // on concurrent ones (seen with `rclone serve sftp` v1.75.2).
                run_all(rc, moves, 1).await?;
            }
        }

        // 3. Move into place. When the destination doesn't exist yet this is one server-side
        //    directory rename; otherwise one server-side move per file.
        progress.send(ProgressEvent::Stage("move-into-place".into()));
        rc.call_job(
            "sync/move",
            json!({
                "srcFs": tmp_fs, "dstFs": dest_fs(dest, &creds, "")?,
                "deleteEmptySrcDirs": true, "_config": config, "_group": group,
            }),
        )
        .await?;
        // Gone already after a directory rename; otherwise empty, and rmdir only removes it then.
        let parent_dest = Destination {
            root: parent.to_owned(),
            ..dest.clone()
        };
        let parent_fs = dest_fs(&parent_dest, &creds, "")?;
        let left = rc
            .call(
                "operations/stat",
                json!({ "fs": parent_fs, "remote": tmp_name }),
            )
            .await?;
        if !left["item"].is_null() {
            rc.call(
                "operations/rmdir",
                json!({ "fs": parent_fs, "remote": tmp_name }),
            )
            .await?;
        }
        Ok(())
    }
}

/// One finished copy: its outcome, its `ItemDone`, and the first `Auth` error in `refused`.
fn record(
    done: std::result::Result<(usize, Result<Value>), tokio::task::JoinError>,
    outcomes: &mut [ItemOutcome],
    progress: &ProgressSink,
    refused: &mut Option<TransportError>,
) {
    let Ok((i, res)) = done else { return };
    let ok = res.is_ok();
    if let Err(e) = res {
        if e.kind == ErrorKind::Auth && refused.is_none() {
            *refused = Some(e.clone());
        }
        outcomes[i] = ItemOutcome::Failed(e);
    }
    progress.send(ProgressEvent::ItemDone { index: i, ok });
}

/// Runs independent rc calls with bounded concurrency; fails on the first error.
async fn run_all(
    rc: &RcClient,
    calls: Vec<(&'static str, Value)>,
    concurrency: usize,
) -> Result<()> {
    let sem = Arc::new(Semaphore::new(concurrency));
    let mut set = JoinSet::new();
    for (method, params) in calls {
        let permit = sem
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore is never closed");
        let rc = rc.clone();
        set.spawn(async move {
            let _permit = permit;
            rc.call(method, params).await
        });
    }
    while let Some(done) = set.join_next().await {
        done.map_err(|e| TransportError::fatal(e.to_string()))??;
    }
    Ok(())
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
        let mut out: Vec<RemoteEntry> = v
            .get("list")
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
        let src_fs = local_fs(&job.source_root)?;
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

        let outcomes = match self.strategy {
            Strategy::PerFile => {
                progress.send(ProgressEvent::Stage("copy".into()));
                self.per_file(&src_fs, &dst_fs, &group, job, &progress)
                    .await
            }
            Strategy::TempThenRename => self
                .temp_then_rename(&src_fs, dest, &group, job, &progress)
                .await
                .map(|()| vec![ItemOutcome::Done; job.items.len()]),
        };
        poller.abort();
        if let Ok(s) = stats::snapshot(rc, &group, Some(&names)).await {
            progress.send(ProgressEvent::Stats(s));
        }
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
                Ok(creds) => self.hash_by_folder(dest, &creds, paths, algo, &mut out).await,
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
    use super::{bwlimit_rate, glob_escape, parse_hashsum_line};
    use filebridge_transport_api::HashAlgo;

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
}
