//! The job runner: stages 4 (transfer), 5 (verify) and 6 (update the tool), with retries.
//! Every state change is persisted, with what the finished stage produced, before the next stage
//! starts, so a crash resumes at the stage it was in; per-entry states mean finished files are
//! never sent again.

use std::collections::HashSet;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use filebridge_integration_api::{IntegrationEvent, IntegrationSink};
use filebridge_store::{EntryState, now_ms};
use filebridge_transport_api::{
    Destination, ErrorKind, HashAlgo, RemotePath, Transport, TransportCaps,
};
use sha2::{Digest as _, Sha256};
use tokio_util::sync::CancellationToken;

use crate::analyse::{bytes_path, source_parent};
use crate::batch::{Batch, Sent, Stop, Tally};
use crate::engine::PREFLIGHT_TIMEOUT;
use crate::inhibit::{SleepGuard, inhibit};
use crate::lifecycle::Outcome;
use crate::repair::RepairEnd;
use crate::sources::source_unavailable;
use crate::{CoreError, Engine, Event, JobKind, JobState, Profile, Result, SendOptions};
use filebridge_model::{JobErrorCode, Op};

const PAGE: u32 = 2_000;
/// The most bytes one verify batch reads ([`verify_batches`]).
const VERIFY_BATCH_BYTES: u64 = 256 << 20;

enum StageEnd<T> {
    Done(T),
    /// A retryable failure: back off, then run the stage again.
    Retry(String),
    Cancelled,
    /// Transfer only: the send window closed; wait for it to open again.
    Held,
    /// The job pauses: trying again won't help until someone acts. The transfer's destination
    /// is full (someone frees space), or a source can't be read (its disk is plugged back in).
    Pause(String, JobErrorCode),
}

struct Ctx {
    job_id: String,
    /// A Repair job renames at the destination instead of sending (`crate::repair`).
    kind: JobKind,
    profile: Profile,
    options: SendOptions,
    sources: Vec<PathBuf>,
    cancel: CancellationToken,
    /// Warnings of the stage running now; saved with the transition that ends it.
    warnings: Vec<String>,
    /// Keeps the computer awake while the job runs; let go while it waits for the window.
    awake: Option<SleepGuard>,
}

impl Engine {
    pub(crate) async fn run(self: &Arc<Self>, job_id: &str, cancel: &CancellationToken) {
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
            kind: JobKind::parse(&row.kind)?,
            profile: self.get_profile(&row.profile_id).await?,
            options: serde_json::from_value(row.options.clone()).unwrap_or_default(),
            sources: self.job_sources(&row).await,
            cancel: cancel.clone(),
            warnings: Vec::new(),
            awake: None,
        };
        let mut state = JobState::parse(&row.state)?;
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            let current = state;
            if current == JobState::Waiting {
                ctx.awake = None;
            } else {
                self.keep_awake(&mut ctx);
            }
            let mut outcome = Outcome::default();
            let next = match current {
                JobState::Retrying => JobState::Transferring,
                JobState::Waiting => {
                    if !self.window_opens(job_id, cancel).await {
                        return Ok(());
                    }
                    JobState::Transferring
                }
                JobState::Transferring if ctx.kind == JobKind::Repair => {
                    // Renames move no data: the send window and verification don't apply.
                    let dir = ctx.options.target_dir.clone();
                    match self
                        .repair_stage(job_id, &ctx.profile, dir.as_deref(), cancel)
                        .await?
                    {
                        RepairEnd::Done(left) => {
                            ctx.warnings.extend(left);
                            JobState::Integrating
                        }
                        RepairEnd::Retry(e) => self.back_off(&mut ctx, current, e).await?,
                        RepairEnd::Cancelled => return Ok(()),
                    }
                }
                JobState::Transferring if self.window_closed() => JobState::Waiting,
                JobState::Transferring => match self.transfer_and_measure(&mut ctx).await? {
                    StageEnd::Done(()) if ctx.options.verify => JobState::Verifying,
                    StageEnd::Done(()) => JobState::Integrating,
                    StageEnd::Retry(e) => self.back_off(&mut ctx, current, e).await?,
                    StageEnd::Cancelled => return Ok(()),
                    StageEnd::Held => JobState::Waiting,
                    StageEnd::Pause(e, code) => return self.pause_for(job_id, e, code).await,
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
                    StageEnd::Pause(e, code) => return self.pause_for(job_id, e, code).await,
                    StageEnd::Held => unreachable!("only the transfer waits for the window"),
                },
                JobState::Integrating => match self.integrate_stage(&mut ctx).await? {
                    StageEnd::Done(()) => {
                        if ctx.kind == JobKind::Repair {
                            let dir = ctx.options.target_dir.as_deref();
                            let failed = self.repair_outcome(job_id, dir).await?.failed;
                            if failed > 0 {
                                ctx.warnings.push(format!(
                                    "{failed} names could not be changed; the Files tab says why"
                                ));
                            }
                        } else {
                            let failed = self.store.manifest_counts(job_id).await?.failed;
                            if failed > 0 {
                                ctx.warnings
                                    .push(format!("{failed} files could not be sent"));
                            }
                        }
                        outcome.warnings = std::mem::take(&mut ctx.warnings);
                        let mut job = self.begin(job_id, Op::Run).await?;
                        return job.complete(outcome).await;
                    }
                    StageEnd::Retry(e) => self.back_off(&mut ctx, current, e).await?,
                    StageEnd::Cancelled => return Ok(()),
                    StageEnd::Held | StageEnd::Pause(..) => {
                        unreachable!("the tool's update reads no source and sends no file")
                    }
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

    /// Pauses the job with an error the UI words itself; Resume carries on where it stopped.
    async fn pause_for(&self, job_id: &str, error: String, code: JobErrorCode) -> Result<()> {
        tracing::warn!(
            job = job_id,
            code = code.as_str(),
            "the job pauses: {error}"
        );
        let mut job = self.begin(job_id, Op::Run).await?;
        job.go_coded(JobState::Paused, error, code).await
    }

    fn keep_awake(&self, ctx: &mut Ctx) {
        if ctx.awake.is_none() && self.options.inhibit_sleep {
            ctx.awake = Some(inhibit("filebridge is sending files"));
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
        self.log(
            Some(&ctx.job_id),
            "retry",
            serde_json::json!({ "attempt": attempt, "in_secs": delay.as_secs(), "error": error }),
        )
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

    /// The transfer stage, then a fresh measurement of the destination (best effort): the send
    /// may have filled it (docs/design/specs/D03).
    async fn transfer_and_measure(&self, ctx: &mut Ctx) -> Result<StageEnd<()>> {
        let end = self.transfer_stage(ctx).await?;
        let _ = self.measure(&ctx.profile, &ctx.job_id).await;
        Ok(end)
    }

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
        let mut batch = Batch {
            store: &self.store,
            events: &self.events,
            transport: transport.as_ref(),
            dest: &ctx.profile.destination,
            job_id: &ctx.job_id,
            sources: &ctx.sources,
            concurrency: ctx.options.concurrency.max(1) as usize,
            files_total: counts.files - counts.skipped,
            bytes_total: counts.bytes - counts.bytes_skipped,
        };
        let mut tally = Tally {
            bytes_done: counts.bytes_done,
            files_done: counts.done,
            ..Tally::default()
        };

        let batches = async {
            loop {
                // Listening before the page is read: a file skipped after this is caught below,
                // one skipped before is no longer Pending.
                let mut skips = self.skips.subscribe();
                let entries = self
                    .store
                    .files_with_transfer_state(
                        &ctx.job_id,
                        EntryState::Pending,
                        None,
                        self.options.batch_size,
                    )
                    .await?;
                if entries.is_empty() {
                    break;
                }
                let in_batch: HashSet<u64> = entries.iter().map(|e| e.entry.idx).collect();
                let stop = async {
                    tokio::select! {
                        _ = ctx.cancel.cancelled() => Stop::Cancelled,
                        _ = self.window_closes() => Stop::Held,
                        _ = skipped_in(&mut skips, &ctx.job_id, &in_batch) => Stop::Skipped,
                    }
                };
                match batch.send(entries, &mut tally, stop).await? {
                    Sent::Answered { retry: None } => {}
                    Sent::Answered { retry: Some(e) } => {
                        self.log_round(&ctx.job_id, &tally).await?;
                        return Ok(StageEnd::Retry(e));
                    }
                    Sent::Full(e) => {
                        self.log_round(&ctx.job_id, &tally).await?;
                        return Ok(StageEnd::Pause(e, JobErrorCode::DestinationFull));
                    }
                    Sent::SourceGone(e) => {
                        self.log_round(&ctx.job_id, &tally).await?;
                        return Ok(StageEnd::Pause(e, JobErrorCode::SourceUnavailable));
                    }
                    Sent::Retry(e) => return Ok(StageEnd::Retry(e)),
                    Sent::Stopped(Stop::Cancelled) => return Ok(StageEnd::Cancelled),
                    Sent::Stopped(Stop::Held) => return Ok(StageEnd::Held),
                    Sent::Stopped(Stop::Skipped) => {
                        // The rest of that batch goes again with the next one; progress counts
                        // from what is stored, now without the skipped file.
                        self.store
                            .reset_transfer_states(
                                &ctx.job_id,
                                &[EntryState::Partial],
                                EntryState::Pending,
                            )
                            .await?;
                        let counts = self.store.manifest_counts(&ctx.job_id).await?;
                        batch.files_total = counts.files - counts.skipped;
                        batch.bytes_total = counts.bytes - counts.bytes_skipped;
                        tally.bytes_done = counts.bytes_done;
                        tally.files_done = counts.done;
                    }
                }
            }
            self.log_round(&ctx.job_id, &tally).await?;
            Ok(StageEnd::Done(()))
        };
        let watch = self.watch_space(&ctx.job_id, transport.as_ref(), &ctx.profile.destination);
        tokio::select! {
            end = batches => end,
            never = watch => match never {},
        }
    }

    /// Measures the destination's free space at the start of the transfer and every
    /// `space_check_every` after, and tells the window how it compares with the bytes still to
    /// send (`Event::DestinationSpace`); the first time they don't fit, the job log says so too.
    /// Only warns: the transfer goes on. A destination that doesn't report its space, or doesn't
    /// answer, is passed over until the next check. Never ends: it runs beside the transfer.
    async fn watch_space(
        &self,
        job_id: &str,
        transport: &dyn Transport,
        dest: &Destination,
    ) -> std::convert::Infallible {
        let mut warned = false;
        loop {
            let measured = tokio::time::timeout(PREFLIGHT_TIMEOUT, transport.preflight(dest)).await;
            if let Ok(Ok(report)) = measured
                && let Some(free_bytes) = report.free_bytes
                && let Ok(counts) = self.store.manifest_counts(job_id).await
            {
                let left_bytes = counts.bytes_left;
                self.emit(Event::DestinationSpace {
                    job_id: job_id.to_owned(),
                    free_bytes,
                    left_bytes,
                });
                if free_bytes < left_bytes && !warned {
                    warned = true;
                    let data =
                        serde_json::json!({ "free_bytes": free_bytes, "left_bytes": left_bytes });
                    if let Err(e) = self.log(Some(job_id), "space_low", data).await {
                        tracing::warn!(job = job_id, "cannot log the low space: {e}");
                    }
                }
            }
            tokio::time::sleep(self.options.space_check_every).await;
        }
    }

    /// One transfer stage run, for the job log: what moved and what didn't.
    async fn log_round(&self, job_id: &str, round: &Tally) -> Result<()> {
        if round.sent == 0 && round.failed == 0 {
            return Ok(());
        }
        self.log(
            Some(job_id),
            "transfer_round",
            serde_json::json!({
                "sent": round.sent, "failed": round.failed, "bytes": round.bytes,
                "first_error": round.first_error,
            }),
        )
        .await
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
        let (first_checked, mut different) = (checked, 0u64);
        let mut after = None;
        loop {
            let page = self.store.entries_page(&ctx.job_id, after, PAGE).await?;
            let Some(last) = page.last() else { break };
            after = Some(last.entry.idx);
            let mut todo = Vec::new();
            for e in page {
                if e.entry.is_dir
                    || e.transfer_state != EntryState::Done
                    || e.verify_state == EntryState::Done
                {
                    continue;
                }
                let Some(dest) = e.dest_path.clone() else {
                    continue;
                };
                let root = ctx
                    .sources
                    .get(e.entry.source as usize)
                    .map(|s| source_parent(s))
                    .ok_or_else(|| CoreError::Invalid("unknown source".into()))?;
                let local = root.join(bytes_path(&e.entry.raw_path));
                todo.push((e.entry.idx, e.entry.size, local, RemotePath::new(dest)?));
            }
            for batch in verify_batches(todo, self.options.batch_size.max(1) as usize) {
                if ctx.cancel.is_cancelled() {
                    return Ok(StageEnd::Cancelled);
                }
                // The source is read while the destination answers: both take a while for big
                // files, and they don't wait for each other.
                let local_paths: Vec<_> = batch.iter().map(|(_, _, l, _)| l.clone()).collect();
                let local = tokio::task::spawn_blocking(move || {
                    local_paths
                        .iter()
                        .map(|p| sha256_file(p))
                        .collect::<Result<Vec<_>>>()
                });
                let remote_paths: Vec<_> = batch.iter().map(|(_, _, _, r)| r.clone()).collect();
                let remote =
                    transport.hash_many(&ctx.profile.destination, &remote_paths, HashAlgo::Sha256);
                let (local, remote) = tokio::join!(local, remote);
                let local = match local
                    .map_err(|err| CoreError::Invalid(format!("hash task failed: {err}")))?
                {
                    Ok(local) => local,
                    // A source that can't be read any more pauses the job, like in the transfer.
                    Err(err) => {
                        let sources = ctx.sources.clone();
                        let gone = tokio::task::spawn_blocking(move || {
                            sources.iter().find_map(|s| source_unavailable(s))
                        })
                        .await
                        .ok()
                        .flatten();
                        return match gone {
                            Some(why) => Ok(StageEnd::Pause(why, JobErrorCode::SourceUnavailable)),
                            None => Err(err),
                        };
                    }
                };
                if remote.len() != batch.len() {
                    return Err(CoreError::Invalid(format!(
                        "the transport answered {} hashes for {} files",
                        remote.len(),
                        batch.len()
                    )));
                }
                let mut updates = Vec::new();
                for (((idx, ..), local), remote) in batch.into_iter().zip(local).zip(remote) {
                    checked += 1;
                    let state = match remote {
                        Ok(r) if r.bytes == local => EntryState::Done,
                        Ok(_) => {
                            mismatched.push(idx);
                            different += 1;
                            EntryState::Mismatch
                        }
                        Err(err) if err.is_retryable() => {
                            self.store
                                .set_verify_results(&ctx.job_id, "sha256", updates)
                                .await?;
                            return Ok(StageEnd::Retry(err.message));
                        }
                        // Gone from the destination: send it again.
                        Err(err) if err.kind == ErrorKind::NotFound => {
                            mismatched.push(idx);
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
                    updates.push((idx, state, Some(local)));
                    self.emit(Event::VerifyProgress {
                        job_id: ctx.job_id.clone(),
                        checked,
                        total,
                        mismatches: mismatched.len() as u64,
                    });
                }
                self.store
                    .set_verify_results(&ctx.job_id, "sha256", updates)
                    .await?;
            }
        }
        self.log(
            Some(&ctx.job_id),
            "verify",
            serde_json::json!({
                "checked": checked - first_checked,
                "mismatch": different,
                "missing": mismatched.len() as u64 - different,
                "unverifiable": unverifiable,
            }),
        )
        .await?;
        if unverifiable > 0 {
            ctx.warnings.push(format!(
                "{unverifiable} files could not be verified ({})",
                first_error.unwrap_or_default()
            ));
        }
        Ok(StageEnd::Done(mismatched))
    }

    // ---- stage 6 -----------------------------------------------------------------------

    /// The destination paths of the files sent: what stage 6 is told changed.
    async fn sent_files(&self, job_id: &str) -> Result<Vec<RemotePath>> {
        let mut changed = Vec::new();
        let mut after = None;
        loop {
            let page = self.store.entries_page(job_id, after, PAGE).await?;
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
        Ok(changed)
    }

    async fn integrate_stage(&self, ctx: &mut Ctx) -> Result<StageEnd<()>> {
        let Some(id) = ctx.profile.integration.clone() else {
            return Ok(StageEnd::Done(()));
        };
        let integration = self.registry.integration(&id)?;
        let job_ctx = self.job_context(&ctx.job_id, &ctx.profile)?;

        let changed = if ctx.kind == JobKind::Repair {
            self.repair_outcome(&ctx.job_id, ctx.options.target_dir.as_deref())
                .await?
                .changed
        } else {
            self.sent_files(&ctx.job_id).await?
        };
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

/// Resolves when one of `in_batch`'s files of job `job_id` is skipped (`Engine::skip_entry`).
/// Never resolves if the channel closes: the engine is going away and the batch ends anyway.
async fn skipped_in(
    skips: &mut tokio::sync::broadcast::Receiver<(String, u64)>,
    job_id: &str,
    in_batch: &HashSet<u64>,
) {
    use tokio::sync::broadcast::error::RecvError;
    loop {
        match skips.recv().await {
            Ok((job, idx)) if job == job_id && in_batch.contains(&idx) => return,
            Ok(_) => {}
            // Missed some: stop to be safe; the batch restarts without whatever was skipped.
            Err(RecvError::Lagged(_)) => return,
            Err(RecvError::Closed) => std::future::pending::<()>().await,
        }
    }
}

/// Files to verify, `(idx, size, local path, destination path)`, in batches of at most `files`
/// files and [`VERIFY_BATCH_BYTES`] (but at least one file): one `hash_many` call each, so a
/// pause is heard and progress shown between them.
fn verify_batches(
    todo: Vec<(u64, u64, PathBuf, RemotePath)>,
    files: usize,
) -> Vec<Vec<(u64, u64, PathBuf, RemotePath)>> {
    let mut batches: Vec<Vec<_>> = Vec::new();
    let mut bytes = 0;
    for item in todo {
        match batches.last_mut() {
            Some(b) if b.len() < files && bytes + item.1 <= VERIFY_BATCH_BYTES => {
                bytes += item.1;
                b.push(item);
            }
            _ => {
                bytes = item.1;
                batches.push(vec![item]);
            }
        }
    }
    batches
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_batches_cap_files_and_bytes() {
        let item = |idx: u64, size: u64| {
            let dest = RemotePath::new(format!("f{idx}")).unwrap();
            (idx, size, PathBuf::from(format!("f{idx}")), dest)
        };
        let idxs = |b: &[Vec<(u64, u64, PathBuf, RemotePath)>]| -> Vec<Vec<u64>> {
            b.iter().map(|b| b.iter().map(|i| i.0).collect()).collect()
        };
        let small = (0..5).map(|i| item(i, 10)).collect();
        assert_eq!(
            idxs(&verify_batches(small, 2)),
            [vec![0, 1], vec![2, 3], vec![4]]
        );
        let big = VERIFY_BATCH_BYTES;
        let mixed = vec![item(0, 10), item(1, big), item(2, big - 10), item(3, 10)];
        assert_eq!(
            idxs(&verify_batches(mixed, 100)),
            [vec![0], vec![1], vec![2, 3]],
            "a file bigger than a batch goes alone"
        );
        assert!(verify_batches(Vec::new(), 10).is_empty());
    }
}
