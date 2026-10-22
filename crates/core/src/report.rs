//! A finished job's report: its summary and every entry with both paths, size, status, the name
//! rules that renamed it and its checksum, as CSV or JSON (docs/design/specs/D09). Streamed page by
//! page, so a 300 000-entry job never sits in memory. The proof of a migration that can leave the
//! app, so it holds no host, user, key path or keyring reference.

use std::io::Write;

use filebridge_model::{
    EntryStatus, ExportResult, FileCategory, Op, ReportFormat, ReportRequest, ReportScope,
};
use filebridge_store::{EntryOrder, EntryQuery, EntryRow, JobRow};
use filebridge_transport_api::Backend;
use serde::Serialize;

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
#[derive(Debug, Clone, Serialize)]
pub struct ReportRow {
    pub source_path: String,
    /// The raw name's bytes in hex, when they aren't valid text (the lossy text isn't exact).
    pub source_path_hex: Option<String>,
    pub destination_path: Option<String>,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<i64>,
    pub category: &'static str,
    pub status: &'static str,
    pub renamed_by: Vec<String>,
    pub sha256: Option<String>,
    pub error: Option<String>,
}

fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
        r.category.to_owned(),
        r.status.to_owned(),
        csv_field(&r.renamed_by.join(";")),
        r.sha256.clone().unwrap_or_default(),
        csv_field(r.error.as_deref().unwrap_or("")),
    ];
    w.write_all(fields.join(",").as_bytes())?;
    w.write_all(b"\r\n")
}

pub fn write_json_begin(w: &mut dyn Write, h: &ReportHeader) -> std::io::Result<()> {
    let head = serde_json::json!({
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
    });
    let text = serde_json::to_string(&head)?;
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
    /// Writes a finished job's report to `w`; returns the rows written. Used by
    /// [`Self::export_report`] (a file) and the CLI (stdout).
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
        let total = {
            let c = self.store.manifest_counts(job_id).await?;
            c.files + if include_dirs { c.dirs } else { 0 }
        };
        match format {
            ReportFormat::Csv => write_csv_header(w, &header)?,
            ReportFormat::Json => write_json_begin(w, &header)?,
        }
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
            for r in &page {
                let row = report_row(r, renamed.get(&r.entry.entry.idx).cloned());
                match format {
                    ReportFormat::Csv => write_csv_row(w, &row)?,
                    ReportFormat::Json => write_json_row(w, &row, rows == 0)?,
                }
                rows += 1;
            }
            self.emit(Event::ReportProgress {
                job_id: job_id.to_owned(),
                rows,
                total,
            });
        }
        if format == ReportFormat::Json {
            write_json_end(w)?;
        }
        w.flush()?;
        Ok(rows)
    }

    /// Writes a finished job's report to `req.path`: first to `<path>.part`, then renamed into
    /// place, so a crash never leaves a cut report under the final name.
    pub async fn export_report(&self, job_id: &str, req: ReportRequest) -> Result<ExportResult> {
        let _job = self.begin(job_id, Op::Export).await?;
        let path = std::path::PathBuf::from(&req.path);
        if path.file_name().is_none() {
            return Err(CoreError::Invalid(format!("{} is not a file", req.path)));
        }
        let mut part = path.clone().into_os_string();
        part.push(".part");
        let part = std::path::PathBuf::from(part);
        let file = std::fs::File::create(&part)?;
        let mut w = std::io::BufWriter::new(file);
        let rows = match self
            .write_report(job_id, req.format, req.scope, req.include_dirs, &mut w)
            .await
        {
            Ok(n) => n,
            Err(e) => {
                drop(w);
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        };
        drop(w);
        std::fs::rename(&part, &path)?;
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Ok(ExportResult {
            path: req.path,
            rows,
            bytes,
        })
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
            .as_str(),
        status: EntryStatus::from_states(e.transfer_state.as_str(), e.verify_state.as_str())
            .as_str(),
        renamed_by: renamed_by.unwrap_or_default(),
        sha256: e
            .entry
            .hash
            .as_ref()
            .filter(|_| e.entry.hash_algo.as_deref().is_none_or(|a| a == "sha256"))
            .map(|h| hex(h)),
        error: e.error.clone(),
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
