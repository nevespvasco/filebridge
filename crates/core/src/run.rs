//! The job runner: stages 4 (transfer), 5 (verify) and 6 (update the tool), with retries.
//! Every state change is persisted, with what the finished stage produced, before the next stage
//! starts, so a crash resumes at the stage it was in; per-entry states mean finished files are
//! never sent again.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use filebridge_integration_api::{IntegrationEvent, IntegrationSink};
use filebridge_store::{EntryState, ManifestEntry, now_ms};
use filebridge_transport_api::{
    ErrorKind, HashAlgo, ItemOutcome, ProgressEvent, ProgressSink, RemotePath, TransferItem,
    TransferJob, TransferOptions, TransportCaps,
};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;

use crate::analyse::{bytes_path, source_parent};
use crate::inhibit::{SleepGuard, inhibit};
use crate::lifecycle::{Op, Outcome};
use crate::{CoreError, Engine, Event, JobState, Profile, Result, SendOptions};

const PAGE: u32 = 2_000;

enum StageEnd<T> {
    Done(T),
    /// A retryable failure: back off, then run the stage again.
    Retry(String),
    Cancelled,
}

struct Ctx {
    job_id: String,
    profile: Profile,
    options: SendOptions,
    sources: Vec<PathBuf>,
    cancel: CancellationToken,
    /// Warnings of the stage running now; saved with the transition that ends it.
    warnings: Vec<String>,
}

impl Engine {
    pub(crate) async fn run(self: &Arc<Self>, job_id: &str, cancel: &CancellationToken) {
        let _guard: Option<SleepGuard> = self
            .options
            .inhibit_sleep
            .then(|| inhibit("filebridge is sending files"));
        if let Err(e) = self.run_inner(job_id, cancel).await {
            if cancel.is_cancelled() {
                return;
            }
            tracing::error!(job = job_id, "job failed: {e}");
            let failed = async {
                let mut job = self.begin(job_id, Op::Run).await?;
                job.go(JobState::Failed, Some(e.to_string())).await
            };
            if let Err(e2) = failed.await {
                tracing::error!(job = job_id, "cannot record the failure: {e2}");
            }
        }
    }

    async fn run_inner(&self, job_id: &str, cancel: &CancellationToken) -> Result<()> {
        let row = self.store.get_job(job_id).await?;
        let mut ctx = Ctx {
            job_id: job_id.to_owned(),
            profile: self.get_profile(&row.profile_id).await?,
            options: serde_json::from_value(row.options.clone()).unwrap_or_default(),
            sources: self.job_sources(&row).await,
            cancel: cancel.clone(),
            warnings: Vec::new(),
        };
        let mut state = JobState::parse(&row.state)?;
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let current = state;
            let mut outcome = Outcome::default();
            let next = match current {
                JobState::Retrying => JobState::Transferring,
                JobState::Transferring => match self.transfer_stage(&mut ctx).await? {
                    StageEnd::Done(()) if ctx.options.verify => JobState::Verifying,
                    StageEnd::Done(()) => JobState::Integrating,
                    StageEnd::Retry(e) => self.back_off(&mut ctx, current, e).await?,
                    StageEnd::Cancelled => return Ok(()),
                },
                JobState::Verifying => match self.verify_stage(&mut ctx).await? {
                    StageEnd::Done(mismatched) if mismatched.is_empty() => JobState::Integrating,
                    StageEnd::Done(mismatched) => {
                        let rounds = 1 + self.begin(job_id, Op::Run).await?.verify_rounds();
                        if rounds >= self.options.max_verify_rounds {
                            ctx.warnings.push(format!(
                                "{} files still differ after {rounds} verification rounds",
                                mismatched.len()
                            ));
                            JobState::Integrating
                        } else {
                            // Resend only those files.
                            let n = mismatched.len();
                            self.store
                                .set_transfer_states(
                                    &ctx.job_id,
                                    mismatched
                                        .iter()
                                        .map(|i| (*i, EntryState::Pending, None))
                                        .collect(),
                                )
                                .await?;
                            self.store
                                .set_verify_states(
                                    &ctx.job_id,
                                    mismatched
                                        .into_iter()
                                        .map(|i| (i, EntryState::Pending))
                                        .collect(),
                                )
                                .await?;
                            tracing::warn!(job = job_id, "{n} files differ; sending them again");
                            outcome.verify_round = true;
                            JobState::Transferring
                        }
                    }
                    StageEnd::Retry(e) => self.back_off(&mut ctx, current, e).await?,
                    StageEnd::Cancelled => return Ok(()),
                },
                JobState::Integrating => match self.integrate_stage(&mut ctx).await? {
                    StageEnd::Done(()) => {
                        let failed = self.store.manifest_counts(job_id).await?.failed;
                        if failed > 0 {
                            ctx.warnings
                                .push(format!("{failed} files could not be sent"));
                        }
                        outcome.warnings = std::mem::take(&mut ctx.warnings);
                        let mut job = self.begin(job_id, Op::Run).await?;
                        return job.complete(outcome).await;
                    }
                    StageEnd::Retry(e) => self.back_off(&mut ctx, current, e).await?,
                    StageEnd::Cancelled => return Ok(()),
                },
                other => {
                    return Err(CoreError::Invalid(format!(
                        "a job in state {} can't run",
                        other.as_str()
                    )));
                }
            };
            outcome.warnings = std::mem::take(&mut ctx.warnings);
            let mut job = self.begin(job_id, Op::Run).await?;
            job.go_with(next, None, outcome).await?;
            state = job.state();
        }
    }

    /// Records the failure, waits the backoff delay, and returns the stage to run again. The
    /// failed stage's warnings are dropped: it runs again from the start.
    async fn back_off(&self, ctx: &mut Ctx, stage: JobState, error: String) -> Result<JobState> {
        ctx.warnings.clear();
        let attempt = self.store.get_job(&ctx.job_id).await?.attempt + 1;
        let delay = self.options.backoff.delay(attempt);
        self.store
            .set_job_retry(
                &ctx.job_id,
                attempt,
                Some(now_ms() + delay.as_millis() as i64),
            )
            .await?;
        self.begin(&ctx.job_id, Op::Run)
            .await?
            .go(JobState::Retrying, Some(error.clone()))
            .await?;
        self.emit(Event::RetryScheduled {
            job_id: ctx.job_id.clone(),
            attempt,
            in_secs: delay.as_secs(),
            error,
        });
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = ctx.cancel.cancelled() => return Ok(JobState::Retrying),
        }
        Ok(stage)
    }

    // ---- stage 4 -----------------------------------------------------------------------

    async fn transfer_stage(&self, ctx: &mut Ctx) -> Result<StageEnd<()>> {
        let transport = self
            .registry
            .transport(&ctx.profile.destination.transport)?;
        // Files left in flight by a pause, a backoff or a crash go again; nothing else is
        // sending them now.
        self.store
            .reset_transfer_states(&ctx.job_id, &[EntryState::Partial], EntryState::Pending)
            .await?;
        let counts = self.store.manifest_counts(&ctx.job_id).await?;
        let files_total = counts.files - counts.skipped;
        let bytes_total = counts.bytes;
        let mut base_bytes = counts.bytes_done;
        let mut base_files = counts.done;
        let mut retry: Option<String> = None;

        loop {
            let batch = self
                .store
                .files_with_transfer_state(
                    &ctx.job_id,
                    EntryState::Pending,
                    None,
                    self.options.batch_size,
                )
                .await?;
            if batch.is_empty() {
                break;
            }
            let mut by_source: BTreeMap<u32, Vec<ManifestEntry>> = BTreeMap::new();
            for e in batch {
                by_source.entry(e.entry.source).or_default().push(e);
            }
            for (source, entries) in by_source {
                let mut items = Vec::with_capacity(entries.len());
                let mut kept = Vec::with_capacity(entries.len());
                let mut updates = Vec::new();
                for e in entries {
                    let dest = e
                        .dest_path
                        .clone()
                        .ok_or_else(|| "no destination name".to_owned())
                        .and_then(|p| RemotePath::new(p).map_err(|err| err.message));
                    match dest {
                        Ok(dest) => {
                            items.push(TransferItem {
                                source: bytes_path(&e.entry.raw_path),
                                dest,
                                size: e.entry.size,
                                expected_hash: None,
                            });
                            kept.push(e);
                        }
                        Err(msg) => updates.push((e.entry.idx, EntryState::Failed, Some(msg))),
                    }
                }
                self.store
                    .set_transfer_states(&ctx.job_id, std::mem::take(&mut updates))
                    .await?;
                if items.is_empty() {
                    continue;
                }
                let root = ctx
                    .sources
                    .get(source as usize)
                    .map(|s| source_parent(s))
                    .ok_or_else(|| CoreError::Invalid(format!("unknown source {source}")))?;
                let tj = TransferJob {
                    id: format!("{}-{}", ctx.job_id, kept[0].entry.idx),
                    source_root: root,
                    items,
                    options: TransferOptions {
                        concurrency: ctx.options.concurrency.max(1) as usize,
                        bwlimit: ctx.options.bwlimit.clone(),
                    },
                };
                self.store
                    .set_transfer_states(
                        &ctx.job_id,
                        kept.iter()
                            .map(|e| (e.entry.idx, EntryState::Partial, None))
                            .collect(),
                    )
                    .await?;

                let (sink, mut rx) = ProgressSink::channel();
                let events = self.events.clone();
                let store = self.store.clone();
                let job_id = ctx.job_id.clone();
                let idx_of: Vec<u64> = kept.iter().map(|e| e.entry.idx).collect();
                let (bb, bf) = (base_bytes, base_files);
                // Forwards progress, and saves each finished file as soon as the transport
                // reports it, so a crash mid-batch never resends what already arrived.
                let forward = tokio::spawn(async move {
                    let mut finished = Vec::new();
                    let mut tick = tokio::time::interval(Duration::from_millis(500));
                    loop {
                        tokio::select! {
                            ev = rx.recv() => match ev {
                                Some(ProgressEvent::Stats(s)) => {
                                    let bytes_done = bb + s.bytes;
                                    let eta_secs = (s.bytes_per_sec > 0.0).then(|| {
                                        (bytes_total.saturating_sub(bytes_done) as f64
                                            / s.bytes_per_sec) as u64
                                    });
                                    let _ = events.send(Event::TransferProgress {
                                        job_id: job_id.clone(),
                                        bytes_done,
                                        bytes_total,
                                        files_done: bf + s.files_done,
                                        files_total,
                                        bytes_per_sec: s.bytes_per_sec,
                                        eta_secs,
                                    });
                                }
                                Some(ProgressEvent::ItemDone { index, ok: true }) => {
                                    if let Some(idx) = idx_of.get(index) {
                                        finished.push((*idx, EntryState::Done, None));
                                    }
                                }
                                Some(_) => {}
                                None => break,
                            },
                            _ = tick.tick() => {}
                        }
                        if !finished.is_empty()
                            && let Err(e) = store
                                .set_transfer_states(&job_id, std::mem::take(&mut finished))
                                .await
                        {
                            tracing::warn!(job = job_id, "cannot save progress: {e}");
                        }
                    }
                    if !finished.is_empty() {
                        let _ = store.set_transfer_states(&job_id, finished).await;
                    }
                });
                let result = tokio::select! {
                    r = transport.transfer(&ctx.profile.destination, &tj, sink) => r,
                    _ = ctx.cancel.cancelled() => {
                        // Dropping the transfer drops its sink; let the forwarder save what
                        // finished before stopping.
                        let _ = tokio::time::timeout(Duration::from_secs(2), forward).await;
                        return Ok(StageEnd::Cancelled);
                    }
                };
                // The transport dropped its sink, so the forwarder ends once it has drained.
                let _ = tokio::time::timeout(Duration::from_secs(1), forward).await;

                let result = match result {
                    Ok(r) => r,
                    Err(e) if e.is_retryable() => return Ok(StageEnd::Retry(e.message)),
                    Err(e) => return Err(e.into()),
                };
                let mut any_done = false;
                for (e, outcome) in kept.iter().zip(result.items) {
                    match outcome {
                        ItemOutcome::Done => {
                            any_done = true;
                            base_bytes += e.entry.size;
                            base_files += 1;
                            updates.push((e.entry.idx, EntryState::Done, None));
                        }
                        ItemOutcome::Failed(err) => {
                            let path = String::from_utf8_lossy(&e.entry.raw_path).into_owned();
                            self.emit(Event::EntryFailed {
                                job_id: ctx.job_id.clone(),
                                path,
                                error: err.message.clone(),
                                retryable: err.is_retryable(),
                            });
                            if err.is_retryable() {
                                retry.get_or_insert_with(|| err.message.clone());
                                updates.push((e.entry.idx, EntryState::Partial, Some(err.message)));
                            } else {
                                updates.push((e.entry.idx, EntryState::Failed, Some(err.message)));
                            }
                        }
                    }
                }
                self.store.set_transfer_states(&ctx.job_id, updates).await?;
                if any_done {
                    self.store.set_job_retry(&ctx.job_id, 0, None).await?;
                }
            }
            if let Some(e) = retry.take() {
                return Ok(StageEnd::Retry(e));
            }
        }
        Ok(StageEnd::Done(()))
    }

    // ---- stage 5 -----------------------------------------------------------------------

    /// Compares SHA-256 of each sent file with the destination. Returns mismatched entries.
    async fn verify_stage(&self, ctx: &mut Ctx) -> Result<StageEnd<Vec<u64>>> {
        let transport = self
            .registry
            .transport(&ctx.profile.destination.transport)?;
        if !transport
            .capabilities()
            .contains(TransportCaps::HASH_SHA256)
        {
            ctx.warnings
                .push("this destination can't compute SHA-256; verification skipped".into());
            return Ok(StageEnd::Done(Vec::new()));
        }
        let counts = self.store.manifest_counts(&ctx.job_id).await?;
        let total = counts.done;
        let (mut checked, mut mismatched) = (counts.verified_ok, Vec::new());
        let (mut unverifiable, mut first_error) = (0u64, None::<String>);
        let mut after = None;
        loop {
            let page = self.store.entries_page(&ctx.job_id, after, PAGE).await?;
            let Some(last) = page.last() else { break };
            after = Some(last.entry.idx);
            let mut updates = Vec::new();
            for e in page {
                if e.entry.is_dir
                    || e.transfer_state != EntryState::Done
                    || e.verify_state == EntryState::Done
                {
                    continue;
                }
                if ctx.cancel.is_cancelled() {
                    self.store.set_verify_states(&ctx.job_id, updates).await?;
                    return Ok(StageEnd::Cancelled);
                }
                let root = ctx
                    .sources
                    .get(e.entry.source as usize)
                    .map(|s| source_parent(s))
                    .ok_or_else(|| CoreError::Invalid("unknown source".into()))?;
                let local_path = root.join(bytes_path(&e.entry.raw_path));
                let local = tokio::task::spawn_blocking(move || sha256_file(&local_path))
                    .await
                    .map_err(|err| CoreError::Invalid(format!("hash task failed: {err}")))??;
                let Some(dest) = e.dest_path.clone() else {
                    continue;
                };
                let remote = transport
                    .hash(
                        &ctx.profile.destination,
                        &RemotePath::new(dest)?,
                        HashAlgo::Sha256,
                    )
                    .await;
                checked += 1;
                let state = match remote {
                    Ok(r) if r.bytes == local => EntryState::Done,
                    Ok(_) => {
                        mismatched.push(e.entry.idx);
                        EntryState::Mismatch
                    }
                    Err(err) if err.is_retryable() => {
                        self.store.set_verify_states(&ctx.job_id, updates).await?;
                        return Ok(StageEnd::Retry(err.message));
                    }
                    // Gone from the destination: send it again.
                    Err(err) if err.kind == ErrorKind::NotFound => {
                        mismatched.push(e.entry.idx);
                        EntryState::Missing
                    }
                    // The destination couldn't produce a hash. That says nothing about the
                    // file, so it is reported, not resent.
                    Err(err) => {
                        unverifiable += 1;
                        first_error.get_or_insert(err.message);
                        EntryState::Failed
                    }
                };
                updates.push((e.entry.idx, state));
                self.emit(Event::VerifyProgress {
                    job_id: ctx.job_id.clone(),
                    checked,
                    total,
                    mismatches: mismatched.len() as u64,
                });
            }
            self.store.set_verify_states(&ctx.job_id, updates).await?;
        }
        if unverifiable > 0 {
            ctx.warnings.push(format!(
                "{unverifiable} files could not be verified ({})",
                first_error.unwrap_or_default()
            ));
        }
        Ok(StageEnd::Done(mismatched))
    }

    // ---- stage 6 -----------------------------------------------------------------------

    async fn integrate_stage(&self, ctx: &mut Ctx) -> Result<StageEnd<()>> {
        let Some(id) = ctx.profile.integration.clone() else {
            return Ok(StageEnd::Done(()));
        };
        let integration = self.registry.integration(&id)?;
        let job_ctx = self.job_context(&ctx.job_id, &ctx.profile)?;

        let mut changed = Vec::new();
        let mut after = None;
        loop {
            let page = self.store.entries_page(&ctx.job_id, after, PAGE).await?;
            let Some(last) = page.last() else { break };
            after = Some(last.entry.idx);
            for e in page {
                if !e.entry.is_dir
                    && e.transfer_state == EntryState::Done
                    && let Some(p) = e.dest_path
                {
                    changed.push(RemotePath::new(p)?);
                }
            }
        }
        if changed.is_empty() {
            return Ok(StageEnd::Done(()));
        }

        let (sink, mut rx) = IntegrationSink::channel();
        let events = self.events.clone();
        let job_id = ctx.job_id.clone();
        let forward = tokio::spawn(async move {
            let mut step = String::new();
            while let Some(ev) = rx.recv().await {
                let event = match ev {
                    IntegrationEvent::Stage(s) => {
                        step = s;
                        Event::IntegrationProgress {
                            job_id: job_id.clone(),
                            step: step.clone(),
                            done: 0,
                            total: 0,
                            items: 0,
                            current: None,
                            waiting: None,
                        }
                    }
                    IntegrationEvent::Progress(p) => Event::IntegrationProgress {
                        job_id: job_id.clone(),
                        step: p.step,
                        done: p.done,
                        total: p.total,
                        items: p.items,
                        current: p.current,
                        waiting: None,
                    },
                    IntegrationEvent::Waiting(w) => Event::IntegrationProgress {
                        job_id: job_id.clone(),
                        step: step.clone(),
                        done: 0,
                        total: 0,
                        items: 0,
                        current: None,
                        waiting: Some(w),
                    },
                    IntegrationEvent::Problem(_) => continue, // reported from the final report
                };
                let _ = events.send(event);
            }
        });
        let result = tokio::select! {
            r = integration.after_transfer(&job_ctx, &changed, sink) => r,
            _ = ctx.cancel.cancelled() => {
                forward.abort();
                return Ok(StageEnd::Cancelled);
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(1), forward).await;

        match result {
            Ok(report) => {
                for p in &report.problems {
                    self.emit(Event::IntegrationProblem {
                        job_id: ctx.job_id.clone(),
                        code: p.code.clone(),
                        message: p.message.clone(),
                        path: p.path.clone(),
                    });
                }
                self.store
                    .append_event(
                        Some(&ctx.job_id),
                        "integration",
                        serde_json::to_value(&report)?,
                    )
                    .await?;
                if !report.is_clean() {
                    ctx.warnings.push(format!(
                        "{id}: {} problems after the update",
                        report.problems.len()
                    ));
                }
                Ok(StageEnd::Done(()))
            }
            Err(e) if e.retryable => Ok(StageEnd::Retry(e.message)),
            Err(e) => {
                // The files are at the destination; only the tool update failed.
                ctx.warnings
                    .push(format!("{id} update failed: {}", e.message));
                Ok(StageEnd::Done(()))
            }
        }
    }
}

fn sha256_file(path: &std::path::Path) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_vec())
}
