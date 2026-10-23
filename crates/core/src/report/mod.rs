//! A finished job's report: its summary and every entry with both paths, size, status, the name
//! rules that renamed it and its checksum, as CSV or JSON (docs/design/specs/D09). Streamed page by
//! page, so a 300 000-entry job never sits in memory. The proof of a migration that can leave the
//! app, so it holds no host, user, key path or keyring reference.
//!
//! [`seal`] adds the tamper-evident export (a digest chain, a final SHA-256, a readable HTML copy)
//! and its check; [`diff`] compares two reports (ADR 0025).

pub mod diff;
mod html;
pub mod seal;

use std::collections::VecDeque;
use std::io::Write;

use filebridge_model::{
    EntryStatus, ExportResult, FileCategory, JobState, Op, ReportChange, ReportChangeKind,
    ReportCheck, ReportDiff, ReportFormat, ReportRequest, ReportScope,
};
use filebridge_store::{EntryOrder, EntryQuery, EntryRow, JobRow};
use filebridge_transport_api::Backend;
use serde::{Deserialize, Serialize};

use crate::{CoreError, Engine, Event, Profile, Result};

/// Entries read from the store per page.
const PAGE: u32 = 1000;
/// Version of the JSON layout; a change that breaks readers bumps it.
pub const JSON_FORMAT: &str = "filebridge-report/1";

/// Where the job went, without anything that reaches it (host, user, keys).
#[derive(Debug, Clone, Serialize)]
pub struct ReportDestination {
    pub profile: String,
    pub transport: String,
    pub backend: &'static str,
    pub root: String,
    pub integration: Option<String>,
}

impl ReportDestination {
    pub fn of(p: &Profile) -> Self {
        Self {
            profile: p.name.clone(),
            transport: p.destination.transport.clone(),
            backend: match p.destination.backend {
                Backend::Local => "local",
                Backend::Sftp { .. } => "sftp",
                Backend::Webdav { .. } => "webdav",
            },
            root: p.destination.root.clone(),
            integration: p.integration.clone(),
        }
    }
}

/// The summary at the top of a report.
#[derive(Debug, Clone, Serialize)]
pub struct ReportHeader {
    pub id: String,
    pub label: Option<String>,
    pub state: String,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub options: serde_json::Value,
    pub destination: ReportDestination,
    pub files: u64,
    pub bytes: u64,
    pub verify_rounds: u32,
    pub warnings: Vec<String>,
    pub app_version: &'static str,
}

/// One entry of a report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportRow {
    pub source_path: String,
    /// The raw name's bytes in hex, when they aren't valid text (the lossy text isn't exact).
    pub source_path_hex: Option<String>,
    pub destination_path: Option<String>,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<i64>,
    pub category: String,
    pub status: String,
    pub renamed_by: Vec<String>,
    pub sha256: Option<String>,
    pub error: Option<String>,
    /// In a sealed report, the digest chain's link at this entry ([`seal`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<String>,
}

impl ReportRow {
    /// The source name's exact bytes: what two reports' entries are matched by.
    pub fn key(&self) -> Vec<u8> {
        self.source_path_hex
            .as_deref()
            .and_then(unhex)
            .unwrap_or_else(|| self.source_path.as_bytes().to_vec())
    }
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// One CSV field: quoted when it holds a separator, a quote, a line break or edge spaces.
fn csv_field(s: &str) -> String {
    let needs = s.contains([',', '"', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ');
    if needs {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

pub fn write_csv_header(w: &mut dyn Write, h: &ReportHeader) -> std::io::Result<()> {
    // A BOM, so spreadsheets read the accents right.
    w.write_all("\u{feff}".as_bytes())?;
    let d = &h.destination;
    let mut lines = vec![
        "# filebridge report".to_owned(),
        format!("# transfer: {}", h.label.as_deref().unwrap_or("")),
        format!("# transfer_id: {}", h.id),
        format!(
            "# destination: {} ({}, root {})",
            d.profile, d.backend, d.root
        ),
        format!("# created: {}", h.created_at),
        format!("# finished: {}", h.finished_at.as_deref().unwrap_or("")),
        format!("# state: {}", h.state),
        format!(
            "# files: {} · bytes: {} · filebridge {}",
            h.files, h.bytes, h.app_version
        ),
    ];
    for warning in &h.warnings {
        lines.push(format!("# warning: {}", warning.replace(['\n', '\r'], " ")));
    }
    for l in lines {
        w.write_all(l.as_bytes())?;
        w.write_all(b"\r\n")?;
    }
    w.write_all(
        b"source_path,source_path_hex,destination_path,is_dir,size,category,status,renamed_by,sha256,error\r\n",
    )
}

pub fn write_csv_row(w: &mut dyn Write, r: &ReportRow) -> std::io::Result<()> {
    let fields = [
        csv_field(&r.source_path),
        csv_field(r.source_path_hex.as_deref().unwrap_or("")),
        csv_field(r.destination_path.as_deref().unwrap_or("")),
        r.is_dir.to_string(),
        r.size.to_string(),
        r.category.clone(),
        r.status.clone(),
        csv_field(&r.renamed_by.join(";")),
        r.sha256.clone().unwrap_or_default(),
        csv_field(r.error.as_deref().unwrap_or("")),
    ];
    w.write_all(fields.join(",").as_bytes())?;
    w.write_all(b"\r\n")
}

/// The JSON report's top-level object without its entries: what a seal's header link covers.
pub fn json_head(h: &ReportHeader) -> serde_json::Value {
    serde_json::json!({
        "format": JSON_FORMAT,
        "transfer": {
            "id": h.id, "label": h.label, "state": h.state,
            "created_at": h.created_at, "finished_at": h.finished_at, "options": h.options,
        },
        "destination": h.destination,
        "counts": { "files": h.files, "bytes": h.bytes },
        "verify": { "algo": "sha256", "rounds": h.verify_rounds },
        "warnings": h.warnings,
        "app_version": h.app_version,
    })
}

pub fn write_json_begin(w: &mut dyn Write, h: &ReportHeader) -> std::io::Result<()> {
    write_json_head(w, &json_head(h))
}

/// `head`'s members, then the opening of the entries array.
fn write_json_head(w: &mut dyn Write, head: &serde_json::Value) -> std::io::Result<()> {
    let text = serde_json::to_string(head)?;
    // Everything but the closing brace, then the entries array.
    w.write_all(&text.as_bytes()[..text.len() - 1])?;
    w.write_all(b",\"entries\":[")
}

pub fn write_json_row(w: &mut dyn Write, r: &ReportRow, first: bool) -> std::io::Result<()> {
    if !first {
        w.write_all(b",")?;
    }
    w.write_all(b"\n")?;
    serde_json::to_writer(&mut *w, r)?;
    Ok(())
}

pub fn write_json_end(w: &mut dyn Write) -> std::io::Result<()> {
    w.write_all(b"\n]}\n")
}

impl Engine {
    /// Calls `each` with every row of a job's report, in scanner order, page by page; returns
    /// the rows. Emits `ReportProgress` after each page.
    async fn for_each_report_row(
        &self,
        job_id: &str,
        scope: ReportScope,
        include_dirs: bool,
        mut each: impl FnMut(ReportRow) -> std::io::Result<()> + Send,
    ) -> Result<u64> {
        let total = {
            let c = self.store.manifest_counts(job_id).await?;
            c.files + if include_dirs { c.dirs } else { 0 }
        };
        let query = EntryQuery {
            states: match scope {
                ReportScope::Problems => EntryStatus::ALL
                    .into_iter()
                    .filter(|s| s.is_problem())
                    .map(|s| {
                        let (t, v) = s.states();
                        (t.to_owned(), v.map(str::to_owned))
                    })
                    .collect(),
                _ => Vec::new(),
            },
            renamed: (scope == ReportScope::Renamed).then_some(true),
            include_dirs,
            order: EntryOrder::Scan,
            ..EntryQuery::default()
        };
        let mut rows = 0u64;
        let mut after = None;
        loop {
            let page = self
                .store
                .entries_filtered(job_id, query.clone(), after, PAGE)
                .await?;
            let Some(last) = page.last() else { break };
            after = Some(last.key());
            for row in self.report_rows(job_id, &page).await? {
                each(row)?;
                rows += 1;
            }
            self.emit(Event::ReportProgress {
                job_id: job_id.to_owned(),
                rows,
                total,
            });
        }
        Ok(rows)
    }

    /// A page of entries as report rows, with the rules that renamed each.
    async fn report_rows(&self, job_id: &str, page: &[EntryRow]) -> Result<Vec<ReportRow>> {
        let renamed = self
            .store
            .renaming_rules(
                job_id,
                page.iter()
                    .filter(|r| r.renamed)
                    .map(|r| r.entry.entry.idx)
                    .collect(),
            )
            .await?;
        Ok(page
            .iter()
            .map(|r| report_row(r, renamed.get(&r.entry.entry.idx).cloned()))
            .collect())
    }

    /// Writes a finished job's report to `w`; returns the rows written. Used by
    /// [`Self::export_report`] (a file) and the CLI (stdout). Not [`ReportFormat::Sealed`],
    /// which is two files.
    pub async fn write_report(
        &self,
        job_id: &str,
        format: ReportFormat,
        scope: ReportScope,
        include_dirs: bool,
        w: &mut (dyn Write + Send),
    ) -> Result<u64> {
        let job = self.store.get_job(job_id).await?;
        let header = self.report_header(&job).await?;
        match format {
            ReportFormat::Csv => write_csv_header(w, &header)?,
            ReportFormat::Json => write_json_begin(w, &header)?,
            ReportFormat::Sealed => {
                return Err(CoreError::Invalid(
                    "a sealed report is two files: give it a file name".into(),
                ));
            }
        }
        let mut first = true;
        let rows = self
            .for_each_report_row(job_id, scope, include_dirs, |row| {
                let first = std::mem::replace(&mut first, false);
                match format {
                    ReportFormat::Csv => write_csv_row(w, &row),
                    _ => write_json_row(w, &row, first),
                }
            })
            .await?;
        if format == ReportFormat::Json {
            write_json_end(w)?;
        }
        w.flush()?;
        Ok(rows)
    }

    /// Writes a finished job's report to `req.path`: first to `<path>.part`, then renamed into
    /// place, so a crash never leaves a cut report under the final name. A sealed report also
    /// writes its readable copy, `req.path` with `.html`.
    pub async fn export_report(&self, job_id: &str, req: ReportRequest) -> Result<ExportResult> {
        let _job = self.begin(job_id, Op::Export).await?;
        let path = std::path::PathBuf::from(&req.path);
        if path.file_name().is_none() {
            return Err(CoreError::Invalid(format!("{} is not a file", req.path)));
        }
        let readable = (req.format == ReportFormat::Sealed).then(|| path.with_extension("html"));
        if readable.as_ref() == Some(&path) {
            return Err(CoreError::Invalid(
                "a sealed report's file name ends in .json; its readable copy is the .html".into(),
            ));
        }
        let mut targets = vec![path.clone()];
        targets.extend(readable.clone());
        let parts: Vec<std::path::PathBuf> = targets.iter().map(|p| part_of(p)).collect();
        let written = self.write_parts(job_id, &req, &parts).await;
        let (rows, seal) = match written {
            Ok(done) => done,
            Err(e) => {
                for p in &parts {
                    let _ = std::fs::remove_file(p);
                }
                return Err(e);
            }
        };
        // The readable copy first: the JSON under its final name means both are complete.
        for (part, target) in parts.iter().zip(&targets).rev() {
            std::fs::rename(part, target)?;
        }
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(ExportResult {
            path: req.path,
            rows,
            bytes,
            readable: readable.map(|p| p.display().to_string()),
            seal,
        })
    }

    /// The report into `parts` (the JSON, then the readable copy when sealed): the rows and the
    /// seal.
    async fn write_parts(
        &self,
        job_id: &str,
        req: &ReportRequest,
        parts: &[std::path::PathBuf],
    ) -> Result<(u64, Option<String>)> {
        let open = |p: &std::path::Path| -> Result<std::io::BufWriter<std::fs::File>> {
            Ok(std::io::BufWriter::new(std::fs::File::create(p)?))
        };
        if req.format != ReportFormat::Sealed {
            let mut w = open(&parts[0])?;
            let rows = self
                .write_report(job_id, req.format, req.scope, req.include_dirs, &mut w)
                .await?;
            w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
            return Ok((rows, None));
        }
        let job = self.store.get_job(job_id).await?;
        let head = json_head(&self.report_header(&job).await?);
        let mut sealer = seal::Sealer::begin(open(&parts[0])?, open(&parts[1])?, &head)?;
        let rows = self
            .for_each_report_row(job_id, req.scope, req.include_dirs, |row| sealer.row(row))
            .await?;
        let (json, html, digest) = sealer.finish()?;
        for w in [json, html] {
            w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        }
        Ok((rows, Some(digest)))
    }

    /// Compares two finished jobs' reports, files matched by source path: the counts, and the
    /// first `limit` changes (only of `kind` when given), in source-path order.
    pub async fn diff_reports(
        &self,
        job_a: &str,
        job_b: &str,
        kind: Option<ReportChangeKind>,
        limit: u32,
    ) -> Result<ReportDiff> {
        let mut changes = Vec::new();
        let mut more = false;
        let mut diff = self
            .diff_reports_with(job_a, job_b, |c| {
                if kind.is_none_or(|k| c.kinds.contains(&k)) {
                    if changes.len() < limit as usize {
                        changes.push(c);
                    } else {
                        more = true;
                    }
                }
            })
            .await?;
        diff.changes = changes;
        diff.more = more;
        Ok(diff)
    }

    /// [`Self::diff_reports`] with every change handed to `each` (the CLI prints them as they
    /// come); the answer's `changes` stays empty.
    pub async fn diff_reports_with(
        &self,
        job_a: &str,
        job_b: &str,
        each: impl FnMut(ReportChange) + Send,
    ) -> Result<ReportDiff> {
        let mut jobs = Vec::new();
        for id in [job_a, job_b] {
            let job = self.store.get_job(id).await?;
            let state = JobState::parse(&job.state)?;
            if !Op::Export.allowed_in(state) {
                return Err(CoreError::Invalid(format!(
                    "transfer {id} hasn't finished: only finished transfers are compared"
                )));
            }
            jobs.push(job);
        }
        let mut a = Cursor::job(job_a);
        let mut b = Cursor::job(job_b);
        let mut d = diff::Differ::new(each);
        loop {
            let (ka, kb) = (
                self.peek(&mut a).await?.map(|r| r.0.clone()),
                self.peek(&mut b).await?.map(|r| r.0.clone()),
            );
            match diff::step(ka.as_deref(), kb.as_deref()) {
                None => break,
                Some((ta, tb)) => {
                    let ra = if ta { a.next() } else { None };
                    let rb = if tb { b.next() } else { None };
                    d.pair(ra, rb);
                }
            }
        }
        let mut out = d.finish();
        out.same_sources = jobs[0].sources == jobs[1].sources;
        out.same_profile = jobs[0].profile_id == jobs[1].profile_id;
        Ok(out)
    }

    /// The cursor's next row, reading its job's next page when its buffer is empty.
    async fn peek<'c>(&self, c: &'c mut Cursor) -> Result<Option<&'c (Vec<u8>, ReportRow)>> {
        if c.buf.is_empty()
            && let Some(job) = c.job.clone()
        {
            let page = self.store.files_by_path(&job, c.after.take(), PAGE).await?;
            match page.last() {
                None => c.job = None,
                Some(last) => {
                    c.after = Some((last.entry.entry.raw_path.clone(), last.entry.entry.idx));
                    let rows = self.report_rows(&job, &page).await?;
                    c.buf.extend(
                        page.iter()
                            .map(|r| r.entry.entry.raw_path.clone())
                            .zip(rows),
                    );
                }
            }
        }
        Ok(c.buf.front())
    }

    /// Checks a sealed report file (the JSON or its readable copy) on this machine.
    pub async fn check_report(&self, path: &str) -> Result<ReportCheck> {
        let path = std::path::PathBuf::from(path);
        Ok(tokio::task::spawn_blocking(move || seal::check_file(&path))
            .await
            .map_err(|e| CoreError::Invalid(e.to_string()))??)
    }

    async fn report_header(&self, job: &JobRow) -> Result<ReportHeader> {
        let profile = self.profile_for_history(&job.profile_id).await?;
        let counts = self.store.manifest_counts(&job.id).await?;
        Ok(ReportHeader {
            id: job.id.clone(),
            label: job.label.clone(),
            state: job.state.clone(),
            created_at: rfc3339(job.created_at),
            finished_at: job.finished_at.map(rfc3339),
            options: job.options.clone(),
            destination: ReportDestination::of(&profile),
            files: counts.files,
            bytes: counts.bytes,
            verify_rounds: job.verify_rounds,
            warnings: job.warnings.clone(),
            app_version: env!("CARGO_PKG_VERSION"),
        })
    }
}

/// One job's report rows in source-path order, read a page at a time.
struct Cursor {
    /// `None` once its last page was read.
    job: Option<String>,
    after: Option<(Vec<u8>, u64)>,
    buf: VecDeque<(Vec<u8>, ReportRow)>,
}

impl Cursor {
    fn job(id: &str) -> Self {
        Self {
            job: Some(id.to_owned()),
            after: None,
            buf: VecDeque::new(),
        }
    }

    fn next(&mut self) -> Option<ReportRow> {
        self.buf.pop_front().map(|(_, r)| r)
    }
}

fn part_of(path: &std::path::Path) -> std::path::PathBuf {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    part.into()
}

fn report_row(r: &EntryRow, renamed_by: Option<Vec<String>>) -> ReportRow {
    let e = &r.entry;
    let raw = &e.entry.raw_path;
    let source_path_hex = std::str::from_utf8(raw).is_err().then(|| hex(raw));
    ReportRow {
        source_path: String::from_utf8_lossy(raw).into_owned(),
        source_path_hex,
        destination_path: e.dest_path.clone(),
        is_dir: e.entry.is_dir,
        size: e.entry.size,
        mtime: e.entry.mtime,
        category: FileCategory::parse(&e.entry.category)
            .unwrap_or(FileCategory::Other)
            .as_str()
            .to_owned(),
        status: EntryStatus::from_states(e.transfer_state.as_str(), e.verify_state.as_str())
            .as_str()
            .to_owned(),
        renamed_by: renamed_by.unwrap_or_default(),
        sha256: e
            .entry
            .hash
            .as_ref()
            .filter(|_| e.entry.hash_algo.as_deref().is_none_or(|a| a == "sha256"))
            .map(|h| hex(h)),
        error: e.error.clone(),
        chain: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_fields_are_quoted_when_needed() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field(" edge"), "\" edge\"");
        assert_eq!(csv_field("line\nbreak"), "\"line\nbreak\"");
    }

    #[test]
    fn times_are_rfc3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
    }
}
