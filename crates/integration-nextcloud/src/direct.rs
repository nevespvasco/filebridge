//! Direct mode: preflight and after-transfer over a [`RemoteShell`] on the SSH host (the Proxmox
//! node, or the machine Nextcloud runs on).
//!
//! Paths: a `RemotePath` is relative to the job's destination root, which must sit inside the
//! user's files folder (`<datadir>/<user>/files`, host or container view). "Files-relative"
//! below means relative to that folder; it is what `occ files:scan --path=<user>/files/…` wants.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use filebridge_integration_api::{
    AfterTransferReport, Check, CheckStatus, FixAction, IntegrationError, IntegrationEvent,
    IntegrationSink, JobContext, PreflightReport, Problem, RemotePath, Result, StepProgress,
    TransportCaps,
};
use tokio::sync::mpsc;

use crate::parse::{self, ScanLine};
use crate::profile::{NextcloudDirectProfile, path_join};
use crate::quote::join;
use crate::shell::{RemoteShell, ShellConnector};

/// Tuning for [`after_transfer`].
#[derive(Debug, Clone)]
pub struct DirectOptions {
    /// More folders than this are merged into their parents, to bound `occ` start-ups.
    pub max_scan_folders: usize,
    /// Silence from `occ` longer than this shows "waiting for disk".
    pub quiet_after: Duration,
    /// Upper bound for one remote command line (the remote `sh -c` gets it as one argument).
    pub max_command_bytes: usize,
}

impl Default for DirectOptions {
    fn default() -> Self {
        Self {
            max_scan_folders: 64,
            quiet_after: Duration::from_secs(15),
            max_command_bytes: 96 * 1024,
        }
    }
}

pub const STAGE_PERMISSIONS: &str = "nextcloud.permissions";
pub const STAGE_SCAN: &str = "nextcloud.scan";
pub const WAITING_FOR_DISK: &str = "nextcloud.waiting-for-disk";
pub const PROBLEM_ENCODING: &str = "nextcloud.incompatible-encoding";
pub const PROBLEM_SCAN_ERROR: &str = "nextcloud.scan-error";
pub const PROBLEM_SCAN_FAILED: &str = "nextcloud.scan-failed";

/// The destination root, relative to the user's files folder (`""` when it is that folder).
/// A root that climbs out with `..` (or has `.` or empty segments) is refused: the permission fix
/// runs `chown`/`chmod` as root on the paths built from it.
pub fn files_prefix(profile: &NextcloudDirectProfile, root: &str) -> Result<String> {
    let root = root.trim_end_matches('/');
    for base in [profile.host_files_dir(), profile.container_files_dir()] {
        if let Some(rest) = root.strip_prefix(base.as_str())
            && (rest.is_empty() || rest.starts_with('/'))
        {
            let rest = rest.trim_start_matches('/');
            if !rest.is_empty()
                && rest
                    .split('/')
                    .any(|s| s.is_empty() || s == "." || s == "..")
            {
                return Err(IntegrationError::fatal(format!(
                    "destination root {root:?} has an empty, \".\" or \"..\" segment"
                )));
            }
            return Ok(rest.to_string());
        }
    }
    Err(IntegrationError::fatal(format!(
        "destination root {root:?} is not inside Nextcloud's files folder {:?}",
        profile.host_files_dir()
    )))
}

fn rel_join(a: &str, b: &str) -> String {
    match (a, b) {
        ("", b) => b.to_string(),
        (a, "") => a.to_string(),
        (a, b) => format!("{a}/{b}"),
    }
}

fn depth(folder: &str) -> usize {
    if folder.is_empty() {
        0
    } else {
        folder.split('/').count()
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(p, _)| p)
}

fn is_within(path: &str, folder: &str) -> bool {
    folder.is_empty()
        || path == folder
        || path
            .strip_prefix(folder)
            .is_some_and(|r| r.starts_with('/'))
}

fn drop_nested(folders: BTreeSet<String>) -> Vec<String> {
    // Sorted order puts every folder right after its ancestors (`a` < `a/b`), but `a b` sorts
    // between `a` and `a/b`, so check against every kept folder, not only the last.
    let mut kept: Vec<String> = Vec::new();
    for f in folders {
        if !kept.iter().any(|k| is_within(&f, k)) {
            kept.push(f);
        }
    }
    kept
}

/// The fewest folders whose recursive scan covers every changed file: each file's parent,
/// minus folders inside another one. Above `max`, the deepest folders are merged into their
/// parents until it fits (`""` = the whole files folder).
pub fn covering_folders(files_rel: &[String], max: usize) -> Vec<String> {
    let mut folders = drop_nested(files_rel.iter().map(|p| parent(p).to_string()).collect());
    while folders.len() > max.max(1) {
        let deepest = folders.iter().map(|f| depth(f)).max().unwrap_or(0);
        if deepest == 0 {
            break;
        }
        folders = drop_nested(
            folders
                .into_iter()
                .map(|f| {
                    if depth(&f) == deepest {
                        parent(&f).to_string()
                    } else {
                        f
                    }
                })
                .collect(),
        );
    }
    folders
}

/// Every changed path plus each directory between it and the files folder, files-relative.
/// Directories the transfer created get the owner too; existing ones already have it.
fn permission_targets(files_rel: &[String]) -> Vec<String> {
    let mut all = BTreeSet::new();
    for p in files_rel {
        all.insert(p.clone());
        let mut dir = parent(p);
        while !dir.is_empty() {
            if !all.insert(dir.to_string()) {
                break;
            }
            dir = parent(dir);
        }
    }
    all.into_iter().collect()
}

/// Splits `args` so that `prefix + args + suffix` stays under `max` bytes once quoted.
fn chunks(args: &[String], fixed: usize, max: usize) -> Vec<&[String]> {
    let mut out = Vec::new();
    let (mut start, mut len) = (0, fixed);
    for (i, a) in args.iter().enumerate() {
        let n = crate::quote::quote(a).len() + 1;
        if i > start && len + n > max {
            out.push(&args[start..i]);
            (start, len) = (i, fixed);
        }
        len += n;
    }
    if start < args.len() {
        out.push(&args[start..]);
    }
    out
}

fn check(report: &mut PreflightReport, id: &str, status: CheckStatus, message: String) {
    report.checks.push(Check {
        id: id.into(),
        status,
        message,
    });
}

pub async fn preflight(
    profile: &NextcloudDirectProfile,
    ctx: &JobContext,
    connector: &dyn ShellConnector,
) -> Result<PreflightReport> {
    let mut report = PreflightReport::default();

    let prefix = match files_prefix(profile, &ctx.destination().root) {
        Ok(p) => p,
        Err(e) => {
            check(
                &mut report,
                "destination-root",
                CheckStatus::Fail,
                e.message,
            );
            return Ok(report);
        }
    };
    let started = Instant::now();
    let shell = match connector.connect(&profile.ssh).await {
        Ok(s) => s,
        Err(e) => {
            check(&mut report, "ssh", CheckStatus::Fail, e.message);
            return Ok(report);
        }
    };
    report.latency_ms = Some(started.elapsed().as_millis() as u64);
    check(
        &mut report,
        "ssh",
        CheckStatus::Ok,
        format!("connected to {}", profile.ssh.host),
    );

    let lost = |report: &mut PreflightReport, e: IntegrationError| {
        report.checks.push(Check {
            id: "ssh".into(),
            status: CheckStatus::Fail,
            message: format!("connection lost: {e}"),
        });
    };

    // Data path exists and the SSH user (who also writes over SFTP) can write to it.
    let files = profile.host_files_dir();
    let target = path_join(&files, &prefix);
    let exists = match shell.run(&profile.host_cmd(&["test", "-d", &files])).await {
        Ok(o) => o.success(),
        Err(e) => {
            lost(&mut report, e);
            return Ok(report);
        }
    };
    if !exists {
        check(
            &mut report,
            "data-path",
            CheckStatus::Fail,
            format!("{files} does not exist"),
        );
        return Ok(report);
    }
    check(
        &mut report,
        "data-path",
        CheckStatus::Ok,
        format!("{files} exists"),
    );
    // Writable by the SSH user itself (no sudo): it is also the user SFTP writes as. The
    // destination root may not exist yet; then its parent, the files folder, must be writable.
    let probe = if prefix.is_empty() {
        files.clone()
    } else {
        match shell.run(&join(&["test", "-d", &target])).await {
            Ok(o) if o.success() => target.clone(),
            Ok(_) => files.clone(),
            Err(e) => {
                lost(&mut report, e);
                return Ok(report);
            }
        }
    };
    match shell.run(&join(&["test", "-w", &probe])).await {
        Ok(o) => {
            report.writable = o.success();
            let (status, message) = if o.success() {
                (
                    CheckStatus::Ok,
                    format!("{probe} is writable by {}", profile.ssh.user),
                )
            } else {
                (
                    CheckStatus::Fail,
                    format!("{} cannot write to {probe}", profile.ssh.user),
                )
            };
            check(&mut report, "writable", status, message);
        }
        Err(e) => {
            lost(&mut report, e);
            return Ok(report);
        }
    }

    // occ answers and is not in maintenance.
    match shell
        .run(&profile.occ_cmd(&["status", "--output=json"]))
        .await
    {
        Ok(o) => match parse::occ_status(&o.stdout) {
            Some(s) if o.success() && s.installed && !s.maintenance && !s.needs_db_upgrade => {
                check(
                    &mut report,
                    "occ",
                    CheckStatus::Ok,
                    format!("Nextcloud {}", s.versionstring),
                )
            }
            Some(s) if s.maintenance || s.needs_db_upgrade => check(
                &mut report,
                "occ",
                CheckStatus::Fail,
                format!(
                    "Nextcloud {} is {}",
                    s.versionstring,
                    if s.maintenance {
                        "in maintenance mode"
                    } else {
                        "waiting for a database upgrade"
                    }
                ),
            ),
            _ => check(
                &mut report,
                "occ",
                CheckStatus::Fail,
                format!("`occ status` failed: {}", o.short_error()),
            ),
        },
        Err(e) => {
            lost(&mut report, e);
            return Ok(report);
        }
    }

    // Free space on the data volume.
    match shell.run(&profile.host_cmd(&["df", "-Pk", &files])).await {
        Ok(o) => match parse::df_available(&o.stdout).filter(|_| o.success()) {
            Some(free) => {
                report.free_bytes = Some(free);
                check(
                    &mut report,
                    "free-space",
                    CheckStatus::Ok,
                    format!("{free} bytes free"),
                );
            }
            None => check(
                &mut report,
                "free-space",
                CheckStatus::Warn,
                format!("could not read free space: {}", o.short_error()),
            ),
        },
        Err(e) => lost(&mut report, e),
    }
    Ok(report)
}

pub async fn after_transfer(
    profile: &NextcloudDirectProfile,
    ctx: &JobContext,
    changed: &[RemotePath],
    sink: &IntegrationSink,
    connector: &dyn ShellConnector,
    opts: &DirectOptions,
) -> Result<AfterTransferReport> {
    let mut report = AfterTransferReport::default();
    if changed.is_empty() {
        return Ok(report);
    }
    if changed.iter().any(|p| p.as_str().contains('\0')) {
        return Err(IntegrationError::fatal("changed path contains a NUL byte"));
    }
    let prefix = files_prefix(profile, &ctx.destination().root)?;
    let files_rel: Vec<String> = changed
        .iter()
        .map(|p| rel_join(&prefix, p.as_str()))
        .collect();
    let shell = connector.connect(&profile.ssh).await?;

    let need_chown = !ctx.caps.contains(TransportCaps::CHOWN);
    let need_chmod = !ctx.caps.contains(TransportCaps::CHMOD);
    if need_chown || need_chmod {
        sink.send(IntegrationEvent::Stage(STAGE_PERMISSIONS.into()));
        fix_permissions(
            profile,
            shell.as_ref(),
            &files_rel,
            need_chown,
            need_chmod,
            opts,
        )
        .await?;
    }

    sink.send(IntegrationEvent::Stage(STAGE_SCAN.into()));
    let folders = covering_folders(&files_rel, opts.max_scan_folders);
    let mut scan = Scan {
        profile,
        prefix: &prefix,
        sink,
        total: folders.len() as u64,
        items: 0,
        problems: Vec::new(),
    };
    for (i, folder) in folders.iter().enumerate() {
        scan.folder(shell.as_ref(), i as u64, folder, opts.quiet_after)
            .await?;
        report
            .updated
            .push(rel_join(&format!("{}/files", profile.user), folder));
    }
    sink.send(IntegrationEvent::Progress(StepProgress {
        step: STAGE_SCAN.into(),
        done: scan.total,
        total: scan.total,
        items: scan.items,
        current: None,
    }));
    report.items = scan.items;
    report.problems = scan.problems;
    Ok(report)
}

async fn fix_permissions(
    profile: &NextcloudDirectProfile,
    shell: &dyn RemoteShell,
    files_rel: &[String],
    chown: bool,
    chmod: bool,
    opts: &DirectOptions,
) -> Result<()> {
    let base = profile.host_files_dir();
    let paths: Vec<String> = permission_targets(files_rel)
        .iter()
        .map(|p| path_join(&base, p))
        .collect();
    let owner = profile.owner.chown_spec();
    let chmod_tail = [
        "-maxdepth",
        "0",
        "-type",
        "d",
        "-exec",
        "chmod",
        &profile.dir_mode,
        "{}",
        "+",
        "-o",
        "-type",
        "f",
        "-exec",
        "chmod",
        &profile.file_mode,
        "{}",
        "+",
    ];
    let fixed =
        profile.host_cmd(&["chown", "-h", "--", &owner]).len() + join(&chmod_tail).len() + 16;
    for chunk in chunks(&paths, fixed, opts.max_command_bytes) {
        if chown {
            let mut argv = vec!["chown", "-h", "--", &owner];
            argv.extend(chunk.iter().map(String::as_str));
            crate::shell::run_ok(shell, &profile.host_cmd(&argv), "setting the owner").await?;
        }
        if chmod {
            // `find -maxdepth 0` tells files from directories without descending into them.
            let mut argv = vec!["find"];
            argv.extend(chunk.iter().map(String::as_str));
            argv.extend(chmod_tail);
            crate::shell::run_ok(shell, &profile.host_cmd(&argv), "setting permissions").await?;
        }
    }
    Ok(())
}

struct Scan<'a> {
    profile: &'a NextcloudDirectProfile,
    prefix: &'a str,
    sink: &'a IntegrationSink,
    total: u64,
    items: u64,
    problems: Vec<Problem>,
}

/// How often (in files) scan progress is sent.
const PROGRESS_EVERY: u64 = 100;

impl Scan<'_> {
    /// Nextcloud path (`/<user>/files/…`) → destination-relative when inside the destination,
    /// files-relative otherwise.
    fn display_path(&self, nc_path: &str) -> String {
        let files_root = format!("/{}/files/", self.profile.user);
        let rel = nc_path.strip_prefix(&files_root).unwrap_or(nc_path);
        if self.prefix.is_empty() {
            return rel.to_string();
        }
        rel.strip_prefix(self.prefix)
            .and_then(|r| r.strip_prefix('/'))
            .unwrap_or(rel)
            .to_string()
    }

    fn problem(&mut self, p: Problem) {
        self.sink.send(IntegrationEvent::Problem(p.clone()));
        self.problems.push(p);
    }

    async fn folder(
        &mut self,
        shell: &dyn RemoteShell,
        index: u64,
        folder: &str,
        quiet_after: Duration,
    ) -> Result<()> {
        let total = self.total;
        let progress = move |items, current: &str| {
            IntegrationEvent::Progress(StepProgress {
                step: STAGE_SCAN.into(),
                done: index,
                total,
                items,
                current: Some(current.to_string()),
            })
        };
        let scan_path = rel_join(&format!("{}/files", self.profile.user), folder);
        self.sink.send(progress(self.items, &scan_path));
        let cmd = self
            .profile
            .occ_cmd(&["files:scan", "-v", &format!("--path={scan_path}")]);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let run = shell.run_lines(&cmd, tx);
        let mut seen_files = 0u64;
        let mut lines_seen: Vec<ScanLine> = Vec::new();
        let base_items = self.items;
        let sink = self.sink;
        let consume = async {
            let mut waiting = false;
            loop {
                match tokio::time::timeout(quiet_after, rx.recv()).await {
                    Ok(Some(line)) => {
                        waiting = false;
                        match parse::scan_line(&line) {
                            ScanLine::File(_) => {
                                seen_files += 1;
                                if seen_files.is_multiple_of(PROGRESS_EVERY) {
                                    sink.send(progress(base_items + seen_files, &scan_path));
                                }
                            }
                            ScanLine::Folder(_) | ScanLine::Other => {}
                            other => lines_seen.push(other),
                        }
                    }
                    Ok(None) => break,
                    Err(_) if !waiting => {
                        waiting = true;
                        sink.send(IntegrationEvent::Waiting(WAITING_FOR_DISK.into()));
                    }
                    Err(_) => {}
                }
            }
        };
        let (out, ()) = tokio::join!(run, consume);
        let out = out?;

        let mut had_error = false;
        for line in lines_seen {
            match line {
                ScanLine::Encoding(path) => self.problem(Problem {
                    code: PROBLEM_ENCODING.into(),
                    message: format!(
                        "\"{path}\" will not be accessible due to incompatible encoding"
                    ),
                    path: Some(self.display_path(&path)),
                    retryable: false,
                    fix: Some(FixAction::RepairDestination),
                }),
                ScanLine::Error { message, retryable } => {
                    had_error = true;
                    self.problem(Problem {
                        code: PROBLEM_SCAN_ERROR.into(),
                        message,
                        path: Some(scan_path.clone()),
                        retryable,
                        fix: Some(FixAction::Retry),
                    });
                }
                ScanLine::File(_) | ScanLine::Folder(_) | ScanLine::Other => {}
            }
        }
        if !out.success() && !had_error {
            self.problem(Problem {
                code: PROBLEM_SCAN_FAILED.into(),
                message: out.short_error(),
                path: Some(scan_path.clone()),
                retryable: true,
                fix: Some(FixAction::Retry),
            });
        }
        self.items += parse::scan_totals(&out.stdout).map_or(seen_files, |t| t.files);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::tests::sample;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn prefix_from_host_or_container_root() {
        let p = sample();
        assert_eq!(
            files_prefix(&p, "/tank/nextcloud-data/alice/files").unwrap(),
            ""
        );
        assert_eq!(
            files_prefix(&p, "/tank/nextcloud-data/alice/files/Migração/").unwrap(),
            "Migração"
        );
        assert_eq!(
            files_prefix(&p, "/mnt/ncdata/alice/files/A B").unwrap(),
            "A B"
        );
        assert!(files_prefix(&p, "/tank/nextcloud-data/alice/filesX").is_err());
        assert!(files_prefix(&p, "/tank/nextcloud-data/other/files").is_err());
    }

    #[test]
    fn prefix_refuses_climbing_out() {
        let p = sample();
        for root in [
            "/tank/nextcloud-data/alice/files/../../../../etc",
            "/tank/nextcloud-data/alice/files/a/..",
            "/tank/nextcloud-data/alice/files/./a",
            "/tank/nextcloud-data/alice/files/a//b",
        ] {
            assert!(files_prefix(&p, root).is_err(), "{root}");
        }
    }

    #[test]
    fn covering_drops_nested_and_duplicates() {
        let files = strs(&[
            "Fotos/2024/a.jpg",
            "Fotos/2024/b.jpg",
            "Fotos/c.jpg",
            "Fotos b/x.txt",
            "Fotos/2024/Junho/d.jpg",
            "Docs/IRS/2023.pdf",
            "Docs/IRS 2/2024.pdf",
        ]);
        assert_eq!(
            covering_folders(&files, 64),
            ["Docs/IRS", "Docs/IRS 2", "Fotos", "Fotos b"]
        );
    }

    #[test]
    fn covering_root_file_means_whole_folder() {
        assert_eq!(covering_folders(&strs(&["a.txt", "B/c.txt"]), 64), [""]);
    }

    #[test]
    fn covering_merges_deepest_above_max() {
        let files = strs(&["A/1/x", "A/2/x", "A/3/x", "B/1/x", "C/x"]);
        assert_eq!(covering_folders(&files, 5).len(), 5);
        assert_eq!(covering_folders(&files, 3), ["A", "B", "C"]);
        assert_eq!(covering_folders(&files, 2), [""]);
    }

    #[test]
    fn permission_targets_include_parents_once() {
        assert_eq!(
            permission_targets(&strs(&["A/B/x.jpg", "A/B/y.jpg", "A/z.jpg", "c.txt"])),
            ["A", "A/B", "A/B/x.jpg", "A/B/y.jpg", "A/z.jpg", "c.txt"]
        );
    }

    #[test]
    fn chunks_respect_limit() {
        let args: Vec<String> = (0..10).map(|i| format!("/p/{i:04}")).collect();
        let parts = chunks(&args, 10, 10 + 8 * 3);
        assert_eq!(parts.len(), 4);
        assert_eq!(parts.iter().map(|c| c.len()).sum::<usize>(), 10);
        // One argument longer than the limit still gets its own chunk.
        let long = vec!["x".repeat(100)];
        assert_eq!(chunks(&long, 0, 10).len(), 1);
    }
}
