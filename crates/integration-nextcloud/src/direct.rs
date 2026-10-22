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
    /// In a folder that also holds files the job didn't send, more sent files than this (directly
    /// in it) are scanned with one `--shallow` run of the folder instead of one run per file.
    pub shallow_above: usize,
    /// Silence from `occ` longer than this shows "waiting for disk".
    pub quiet_after: Duration,
    /// Upper bound for one remote command line (the remote `sh -c` gets it as one argument).
    pub max_command_bytes: usize,
}

impl Default for DirectOptions {
    fn default() -> Self {
        Self {
            shallow_above: 16,
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

/// One `occ files:scan` run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanTarget {
    /// A folder holding only sent files, scanned recursively.
    Tree(String),
    /// A folder's own entries, not its subfolders (`--shallow`).
    Shallow(String),
    /// One sent file.
    File(String),
}

impl ScanTarget {
    fn path(&self) -> &str {
        match self {
            Self::Tree(p) | Self::Shallow(p) | Self::File(p) => p,
        }
    }
}

/// Decides how to scan the sent files under `dir`, given how many files `dir` holds on disk.
/// When they are all sent, one recursive scan covers them and nothing else. Otherwise the sent
/// files directly in `dir` are scanned one by one (or with one `--shallow` run above
/// `shallow_above`), and the subfolders holding sent files are returned to be decided the same way.
pub fn plan_folder(
    files_rel: &[String],
    dir: &str,
    on_disk: u64,
    shallow_above: usize,
) -> (Vec<ScanTarget>, Vec<String>) {
    let under: Vec<&String> = files_rel
        .iter()
        .filter(|f| f.as_str() != dir && is_within(f, dir))
        .collect();
    if on_disk <= under.len() as u64 {
        return (vec![ScanTarget::Tree(dir.to_string())], Vec::new());
    }
    let direct: Vec<&String> = under.iter().copied().filter(|f| parent(f) == dir).collect();
    let mut targets = if direct.len() > shallow_above {
        vec![ScanTarget::Shallow(dir.to_string())]
    } else {
        direct
            .iter()
            .map(|f| ScanTarget::File(f.to_string()))
            .collect()
    };
    targets.sort_by(|a, b| a.path().cmp(b.path()));
    let skip = if dir.is_empty() { 0 } else { dir.len() + 1 };
    let children: BTreeSet<String> = under
        .iter()
        .filter_map(|f| {
            let first = f[skip..].split_once('/')?.0;
            Some(rel_join(dir, first))
        })
        .collect();
    (targets, children.into_iter().collect())
}

/// Counts the files under each folder (host paths), stopping at `limit`: the caller only needs
/// to know whether there are more than it sent.
const COUNT_SCRIPT: &str = "while [ $# -gt 1 ]; do find \"$2\" -type f 2>/dev/null | head -n \"$1\" | wc -l; shift 2; done";

/// The command that counts files in `dirs` (`(host path, limit)` pairs), one number per line.
pub fn count_files_cmd(profile: &NextcloudDirectProfile, dirs: &[(String, u64)]) -> String {
    let mut argv = vec![
        "sh".to_string(),
        "-c".to_string(),
        COUNT_SCRIPT.to_string(),
        "sh".to_string(),
    ];
    for (dir, limit) in dirs {
        argv.push(limit.to_string());
        argv.push(dir.clone());
    }
    profile.host_cmd(&argv)
}

/// How many files each folder holds on disk (files-relative folders), up to `limit` each.
async fn files_on_disk(
    profile: &NextcloudDirectProfile,
    shell: &dyn RemoteShell,
    dirs: &[(String, u64)],
    opts: &DirectOptions,
) -> Result<Vec<u64>> {
    let base = profile.host_files_dir();
    let mut counts = Vec::with_capacity(dirs.len());
    let fixed = count_files_cmd(profile, &[]).len();
    let mut batch: Vec<(String, u64)> = Vec::new();
    let mut len = fixed;
    let mut i = 0;
    while i < dirs.len() || !batch.is_empty() {
        if let Some((dir, limit)) = dirs.get(i) {
            let host = path_join(&base, dir);
            let n = crate::quote::quote(&host).len() + 22;
            if batch.is_empty() || len + n <= opts.max_command_bytes {
                batch.push((host, *limit));
                len += n;
                i += 1;
                continue;
            }
        }
        let out = crate::shell::run_ok(
            shell,
            &count_files_cmd(profile, &batch),
            "counting files in the destination",
        )
        .await?;
        let got: Vec<u64> = out
            .stdout
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect();
        if got.len() != batch.len() {
            return Err(IntegrationError::retryable(format!(
                "counting files in the destination: expected {} numbers, got {:?}",
                batch.len(),
                out.stdout
            )));
        }
        counts.extend(got);
        batch.clear();
        len = fixed;
    }
    Ok(counts)
}

/// The scans that index exactly the sent files, starting from the destination root (`prefix`)
/// and descending only into folders that also hold other files. One count command per level.
async fn scan_targets(
    profile: &NextcloudDirectProfile,
    shell: &dyn RemoteShell,
    prefix: &str,
    files_rel: &[String],
    opts: &DirectOptions,
) -> Result<Vec<ScanTarget>> {
    let mut targets = Vec::new();
    let mut level = vec![prefix.to_string()];
    while !level.is_empty() {
        let dirs: Vec<(String, u64)> = level
            .iter()
            .map(|d| {
                let sent = files_rel
                    .iter()
                    .filter(|f| f.as_str() != d && is_within(f, d))
                    .count();
                (d.clone(), sent as u64 + 1)
            })
            .collect();
        let counts = files_on_disk(profile, shell, &dirs, opts).await?;
        let mut next = Vec::new();
        for (dir, on_disk) in level.iter().zip(counts) {
            let (t, children) = plan_folder(files_rel, dir, on_disk, opts.shallow_above);
            targets.extend(t);
            next.extend(children);
        }
        level = next;
    }
    Ok(targets)
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
    let targets = scan_targets(profile, shell.as_ref(), &prefix, &files_rel, opts).await?;
    let mut scan = Scan {
        profile,
        prefix: &prefix,
        sink,
        total: targets.len() as u64,
        items: 0,
        problems: Vec::new(),
    };
    for (i, target) in targets.iter().enumerate() {
        scan.target(shell.as_ref(), i as u64, target, opts.quiet_after)
            .await?;
        report
            .updated
            .push(rel_join(&format!("{}/files", profile.user), target.path()));
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

    async fn target(
        &mut self,
        shell: &dyn RemoteShell,
        index: u64,
        target: &ScanTarget,
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
        let scan_path = rel_join(&format!("{}/files", self.profile.user), target.path());
        self.sink.send(progress(self.items, &scan_path));
        let path_arg = format!("--path={scan_path}");
        let args: &[&str] = match target {
            ScanTarget::Shallow(_) => &["files:scan", "-v", "--shallow", &path_arg],
            ScanTarget::Tree(_) | ScanTarget::File(_) => &["files:scan", "-v", &path_arg],
        };
        let cmd = self.profile.occ_cmd(args);

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
    fn plan_whole_folder_when_only_sent_files_are_there() {
        let files = strs(&["Fotos/a.jpg", "Fotos/2024/b.jpg"]);
        assert_eq!(
            plan_folder(&files, "Fotos", 2, 16),
            (vec![ScanTarget::Tree("Fotos".into())], vec![])
        );
        assert_eq!(
            plan_folder(&files, "", 2, 16),
            (vec![ScanTarget::Tree("".into())], vec![]),
            "an empty destination is scanned whole"
        );
    }

    #[test]
    fn plan_files_and_subfolders_when_others_are_there() {
        let files = strs(&["cv.pdf", "Fotos/a.jpg", "Fotos/2024/b.jpg", "Docs/c.pdf"]);
        assert_eq!(
            plan_folder(&files, "", 7_400, 16),
            (
                vec![ScanTarget::File("cv.pdf".into())],
                vec!["Docs".into(), "Fotos".into()]
            )
        );
        assert_eq!(
            plan_folder(&files, "Fotos", 3, 16),
            (
                vec![ScanTarget::File("Fotos/a.jpg".into())],
                vec!["Fotos/2024".into()]
            )
        );
    }

    #[test]
    fn plan_shallow_above_threshold() {
        let files = strs(&["a b/1", "a b/2", "a b/3", "a b/x/4"]);
        assert_eq!(
            plan_folder(&files, "a b", 10, 2),
            (
                vec![ScanTarget::Shallow("a b".into())],
                vec!["a b/x".into()]
            )
        );
        assert_eq!(plan_folder(&files, "a b", 10, 3).0.len(), 3);
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
