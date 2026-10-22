use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use filebridge_transport_api::{
    Check, CheckStatus, Destination, Digest, ErrorKind, HashAlgo, ItemOutcome, PreflightReport,
    ProgressEvent, ProgressSink, RemoteEntry, RemotePath, Result, TransferJob, TransferResult,
    Transport, TransportCaps, TransportError,
};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::fs::{dest_fs, join, local_fs, split_root, utf8};
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
}

impl RcloneTransport {
    pub const ID: &'static str = "rclone";

    pub fn new(rcd: Arc<Rcd>, strategy: Strategy) -> Self {
        Self { rcd, strategy }
    }

    fn rc(&self) -> &RcClient {
        self.rcd.client()
    }

    async fn per_file(
        &self,
        src_fs: &str,
        dst_fs: &str,
        group: &str,
        job: &TransferJob,
        progress: &ProgressSink,
    ) -> Vec<ItemOutcome> {
        let mut outcomes = vec![ItemOutcome::Done; job.items.len()];
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
            while let Some(done) = set.try_join_next() {
                record(done, &mut outcomes, progress);
            }
        }
        while let Some(done) = set.join_next().await {
            record(done, &mut outcomes, progress);
        }
        outcomes
    }

    async fn temp_then_rename(
        &self,
        src_fs: &str,
        dest: &Destination,
        group: &str,
        job: &TransferJob,
        progress: &ProgressSink,
    ) -> Result<()> {
        let rc = self.rc();
        let concurrency = job.options.concurrency.max(1);
        let (parent, base) = split_root(&dest.root)?;
        let tmp_name = format!(".{base}.filebridge-tmp-{}", job.id);
        let tmp = Destination {
            root: join(parent, &tmp_name),
            ..dest.clone()
        };
        let tmp_fs = dest_fs(&tmp, "")?;
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
                            "srcFs": dest_fs(&tmp, a)?, "dstFs": dest_fs(&tmp, b)?,
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
                "srcFs": tmp_fs, "dstFs": dest_fs(dest, "")?,
                "deleteEmptySrcDirs": true, "_config": config, "_group": group,
            }),
        )
        .await?;
        // Gone already after a directory rename; otherwise empty, and rmdir only removes it then.
        let parent_dest = Destination {
            root: parent.to_owned(),
            ..dest.clone()
        };
        let parent_fs = dest_fs(&parent_dest, "")?;
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

fn record(
    done: std::result::Result<(usize, Result<Value>), tokio::task::JoinError>,
    outcomes: &mut [ItemOutcome],
    progress: &ProgressSink,
) {
    let Ok((i, res)) = done else { return };
    let ok = res.is_ok();
    if let Err(e) = res {
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
        let started = Instant::now();
        let about = self
            .rc()
            .call("operations/about", json!({ "fs": dest_fs(dest, "")? }))
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

    async fn move_path(
        &self,
        dest: &Destination,
        from: &RemotePath,
        to: &RemotePath,
    ) -> Result<()> {
        let parent_of = |p: &RemotePath| p.parent().map(RemotePath::new).transpose();
        let listed = |dir: Option<RemotePath>| async move {
            match self.list(dest, dir.as_ref(), false).await {
                Err(e) if e.kind == ErrorKind::NotFound => Ok(Vec::new()),
                other => other,
            }
        };
        let source = listed(parent_of(from)?)
            .await?
            .into_iter()
            .find(|e| e.name == from.file_name())
            .ok_or_else(|| TransportError::not_found(format!("{from} is not there")))?;
        if from == to {
            return Ok(());
        }
        // Exact names only: on a server that normalises or ignores case, `to` may be `from`
        // itself under another spelling, and that is not a clash.
        if listed(parent_of(to)?)
            .await?
            .iter()
            .any(|e| e.name == to.file_name())
        {
            return Err(TransportError::fatal(format!("{to} already exists")));
        }
        // Through an intermediate name, as in the temp-folder rename: a server that treats the
        // two spellings as one name would otherwise see "unchanged" and delete the source.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let mid = join(
            from.parent().unwrap_or(""),
            &format!(".fb-move-{}-{nanos}", std::process::id()),
        );
        let root = dest_fs(dest, "")?;
        for (a, b) in [(from.as_str(), mid.as_str()), (mid.as_str(), to.as_str())] {
            if source.is_dir {
                // The target never exists, so rclone does a single server-side DirMove.
                self.rc()
                    .call_job(
                        "sync/move",
                        json!({
                            "srcFs": dest_fs(dest, a)?, "dstFs": dest_fs(dest, b)?,
                            "deleteEmptySrcDirs": true,
                        }),
                    )
                    .await?;
            } else {
                self.rc()
                    .call(
                        "operations/movefile",
                        json!({ "srcFs": root, "srcRemote": a, "dstFs": root, "dstRemote": b }),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn list(
        &self,
        dest: &Destination,
        dir: Option<&RemotePath>,
        dirs_only: bool,
    ) -> Result<Vec<RemoteEntry>> {
        let v = self
            .rc()
            .call(
                "operations/list",
                json!({
                    "fs": dest_fs(dest, "")?,
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
        let rc = self.rc();
        let group = format!("job-{}", job.id);
        let src_fs = local_fs(&job.source_root)?;
        let dst_fs = dest_fs(dest, "")?;
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
                Ok(self
                    .per_file(&src_fs, &dst_fs, &group, job, &progress)
                    .await)
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
        let hash_type = match algo {
            HashAlgo::Sha256 => "sha256",
            HashAlgo::Xxh3_128 => {
                return Err(TransportError::unsupported(
                    "xxh3-128 is not supported by rclone",
                ));
            }
        };
        let fs = dest_fs(dest, path.as_str())?;
        let params =
            |download: bool| json!({ "fs": fs, "hashType": hash_type, "download": download });
        let first_line = |v: &Value| {
            v.get("hashsum")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        // Servers that can't hash (no `sha256sum`, or `rclone serve sftp`) fall back to rclone
        // downloading the file and hashing it locally: slower, but still a real check. rclone
        // reports "hash unsupported" as an empty list (only logged), or as an error.
        let line = match self.rc().call("operations/hashsum", params(false)).await {
            Ok(v) if first_line(&v).is_some() => first_line(&v),
            Ok(_) => None,
            Err(e) if e.kind == ErrorKind::Unsupported => None,
            Err(e) => return Err(e),
        };
        let line = match line {
            Some(l) => l,
            None => {
                let v = self.rc().call("operations/hashsum", params(true)).await?;
                first_line(&v)
                    .ok_or_else(|| TransportError::fatal(format!("no hash returned for {path}")))?
            }
        };
        let hex = line.split_whitespace().next().unwrap_or_default();
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("zz"), 16))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| TransportError::fatal(format!("bad hash {hex:?} for {path}")))?;
        Ok(Digest { algo, bytes })
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

/// rclone's `--bwlimit` form: a plain number is KiB/s, so bytes need the `B` suffix.
fn bwlimit_rate(bytes_per_sec: Option<u64>) -> String {
    match bytes_per_sec {
        Some(n) => format!("{n}B"),
        None => "off".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::bwlimit_rate;

    #[test]
    fn rate_strings() {
        assert_eq!(bwlimit_rate(Some(2_500_000)), "2500000B");
        assert_eq!(bwlimit_rate(None), "off");
    }
}
