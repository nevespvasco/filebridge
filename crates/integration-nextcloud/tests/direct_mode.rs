//! Direct-mode `preflight` and `after_transfer` through the `Integration` trait, over `FakeShell`.

use std::sync::Arc;
use std::time::Duration;

use filebridge_integration_api::{
    CheckStatus, Destination, FixAction, Integration, IntegrationEvent, IntegrationSink,
    JobContext, ProfileView, RemotePath, TransportCaps,
};
use filebridge_integration_nextcloud::direct::{
    PROBLEM_ENCODING, PROBLEM_SCAN_ERROR, PROBLEM_SCAN_FAILED, STAGE_PERMISSIONS, STAGE_SCAN,
    WAITING_FOR_DISK, count_files_cmd,
};
use filebridge_integration_nextcloud::fake::{FakeConnector, FakeShell};
use filebridge_integration_nextcloud::{
    DirectOptions, Nextcloud, NextcloudDirectProfile, NextcloudSettings, Output, Owner, RunAs,
    Runtime, SshTarget,
};
use filebridge_transport_api::Backend;
use tokio::sync::mpsc::UnboundedReceiver;

const OCC_STATUS: &str = include_str!("fixtures/occ_status.json");
const DF: &str = include_str!("fixtures/df.txt");
const SCAN_FOTOS: &str = include_str!("fixtures/occ_files_scan_verbose.txt");

const FILES: &str = "/tank/nextcloud-data/alice/files";

fn profile() -> NextcloudDirectProfile {
    NextcloudDirectProfile {
        ssh: SshTarget {
            host: "pve.lan".into(),
            port: 22,
            user: "root".into(),
            key_file: None,
            known_hosts_file: None,
            key: None,
        },
        host_sudo: false,
        runtime: Runtime::ProxmoxLxc { ctid: 105 },
        install_dir: "/var/www/nextcloud".into(),
        php: "php".into(),
        run_as: RunAs::Su,
        web_user: "www-data".into(),
        data_dir: "/mnt/ncdata".into(),
        host_data_dir: "/tank/nextcloud-data".into(),
        user: "alice".into(),
        owner: Owner {
            uid: 100_033,
            gid: 100_033,
        },
        dir_mode: "750".into(),
        file_mode: "640".into(),
        access: Default::default(),
    }
}

fn ctx(root: &str, caps: TransportCaps) -> JobContext {
    JobContext {
        job_id: "job-7".into(),
        profile: ProfileView {
            id: "p-casa".into(),
            name: "Nextcloud casa".into(),
            destination: Destination {
                transport: "rclone".into(),
                backend: Backend::Local,
                root: root.into(),
            },
            settings: serde_json::to_value(NextcloudSettings::Direct(Box::new(profile()))).unwrap(),
        },
        caps,
        checkpoint: None,
    }
}

fn paths(v: &[&str]) -> Vec<RemotePath> {
    v.iter().map(|p| RemotePath::new(*p).unwrap()).collect()
}

fn setup() -> (Arc<FakeShell>, FakeConnector, Nextcloud) {
    let shell = Arc::new(FakeShell::new());
    let connector = FakeConnector::new(shell.clone());
    let nc = Nextcloud::new(Arc::new(connector.clone()));
    (shell, connector, nc)
}

fn drain(rx: &mut UnboundedReceiver<IntegrationEvent>) -> Vec<IntegrationEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn scan_cmd(path: &str) -> String {
    profile().occ_cmd_stop_on_eof(&["files:scan", "-v", &format!("--path={path}")])
}

/// The file count for `(files-relative folder, limit)` pairs.
fn count_cmd(dirs: &[(&str, u64)]) -> String {
    let dirs: Vec<(String, u64)> = dirs
        .iter()
        .map(|(d, n)| {
            let host = if d.is_empty() {
                FILES.to_string()
            } else {
                format!("{FILES}/{d}")
            };
            (host, *n)
        })
        .collect();
    count_files_cmd(&profile(), &dirs)
}

#[tokio::test]
async fn preflight_all_green() {
    let (shell, connector, nc) = setup();
    let root = format!("{FILES}/Migração");
    shell
        .on(format!("test -d {FILES}"), Output::ok(""))
        .on(format!("test -d '{root}'"), Output::fail(1, ""))
        .on(format!("test -w {FILES}"), Output::ok(""))
        .on(
            "pct exec 105 -- su -s /bin/bash www-data -c 'php /var/www/nextcloud/occ status --output=json'",
            Output::ok(OCC_STATUS),
        )
        .on(format!("df -Pk {FILES}"), Output::ok(DF))
        // No quota (-3 is "unlimited"): the disk's numbers stand.
        .on(
            "pct exec 105 -- su -s /bin/bash www-data -c 'php /var/www/nextcloud/occ user:info alice --output=json'",
            Output::ok(r#"{"user_id":"alice","storage":{"free":1,"used":2,"quota":-3}}"#),
        );
    let report = nc
        .preflight(&ctx(&root, TransportCaps::empty()))
        .await
        .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    let checks: Vec<_> = report
        .checks
        .iter()
        .map(|c| (c.id.as_str(), c.status))
        .collect();
    assert_eq!(
        checks,
        [
            ("ssh", CheckStatus::Ok),
            ("data-path", CheckStatus::Ok),
            ("writable", CheckStatus::Ok),
            ("occ", CheckStatus::Ok),
            ("free-space", CheckStatus::Ok),
        ]
    );
    assert!(report.writable);
    assert_eq!(report.free_bytes, Some(2_729_627_648 * 1024));
    assert_eq!(report.total_bytes, Some(3_904_897_024 * 1024));
    assert_eq!(report.used_bytes, Some(1_175_269_376 * 1024));
    assert!(report.latency_ms.is_some());
    assert_eq!(connector.targets()[0].host, "pve.lan");
}

#[tokio::test]
async fn preflight_failures_are_checks() {
    let (shell, _, nc) = setup();
    shell
        .on(format!("test -d {FILES}"), Output::ok(""))
        .on(format!("test -w {FILES}"), Output::fail(1, ""))
        .on(
            profile().occ_cmd(&["status", "--output=json"]),
            Output::ok(OCC_STATUS.replace("\"maintenance\":false", "\"maintenance\":true")),
        )
        .on(
            format!("df -Pk {FILES}"),
            Output::fail(1, "df: permission denied"),
        );
    let report = nc
        .preflight(&ctx(FILES, TransportCaps::empty()))
        .await
        .unwrap();
    let get = |id: &str| report.checks.iter().find(|c| c.id == id).unwrap();
    assert!(!report.writable);
    assert_eq!(get("writable").status, CheckStatus::Fail);
    assert_eq!(get("occ").status, CheckStatus::Fail);
    assert!(get("occ").message.contains("maintenance"));
    assert_eq!(get("free-space").status, CheckStatus::Warn);
    assert_eq!(report.free_bytes, None);
}

#[tokio::test]
async fn preflight_unreachable_and_wrong_root() {
    let nc = Nextcloud::new(Arc::new(FakeConnector::refusing()));
    let report = nc
        .preflight(&ctx(FILES, TransportCaps::empty()))
        .await
        .unwrap();
    assert_eq!(report.checks[0].id, "ssh");
    assert_eq!(report.checks[0].status, CheckStatus::Fail);

    let (shell, _, nc) = setup();
    let report = nc
        .preflight(&ctx("/tank/other", TransportCaps::empty()))
        .await
        .unwrap();
    assert_eq!(report.checks[0].id, "destination-root");
    assert!(
        shell.ran().is_empty(),
        "nothing runs when the root is wrong"
    );
}

#[tokio::test]
async fn bad_settings_are_fatal() {
    let (_, _, nc) = setup();
    let mut c = ctx(FILES, TransportCaps::empty());
    c.profile.settings = serde_json::json!({ "mode": "webdav" });
    let err = nc.preflight(&c).await.unwrap_err();
    assert!(!err.retryable);
}

#[tokio::test]
async fn after_transfer_fixes_owner_and_scans_minimal_folders() {
    let (shell, _, nc) = setup();
    let changed = paths(&[
        "Fotos/2024/IMG_0001.jpg",
        "Fotos/2024/IMG 0002 (cópia).jpg",
        "Fotos/Viagem a Óbidos/O'Brien \"festa\".jpg",
        "Fotos/Viagem a Óbidos/mapa.pdf",
        "Fotos/capa.jpg",
    ]);
    // Exact command lines: paths with spaces, quotes and accents are single-quoted. Parent
    // folders change only while the SSH user (uid 0, as SFTP made them) still owns them, mode
    // before owner; the sent files always.
    let dirs = "/tank/nextcloud-data/alice/files/Fotos \
        /tank/nextcloud-data/alice/files/Fotos/2024 \
        '/tank/nextcloud-data/alice/files/Fotos/Viagem a Óbidos'";
    let files = "/tank/nextcloud-data/alice/files/Fotos/2024/IMG_0001.jpg \
        '/tank/nextcloud-data/alice/files/Fotos/2024/IMG 0002 (cópia).jpg' \
        '/tank/nextcloud-data/alice/files/Fotos/Viagem a Óbidos/O'\\''Brien \"festa\".jpg' \
        '/tank/nextcloud-data/alice/files/Fotos/Viagem a Óbidos/mapa.pdf' \
        /tank/nextcloud-data/alice/files/Fotos/capa.jpg";
    let dir_mode = format!("find {dirs} -maxdepth 0 -type d -user 0 -exec chmod 750 '{{}}' +");
    let dir_owner =
        format!("find {dirs} -maxdepth 0 -type d -user 0 -exec chown -h 100033:100033 '{{}}' +");
    let chown = format!("chown -h -- 100033:100033 {files}");
    let find = format!("find {files} -maxdepth 0 -type f -exec chmod 640 '{{}}' +");
    // `occ` runs in the background; a watcher on the SSH channel (stdin) stops it on EOF.
    let scan = "pct exec 105 -- su -s /bin/bash www-data -c 'exec 3<&0; \
        php /var/www/nextcloud/occ files:scan -v --path=alice/files/Fotos </dev/null & p=$!; w=; \
        if [ -p /dev/stdin ] || [ -S /dev/stdin ]; then \
        (while read -r l; do :; done <&3; kill -TERM $p) >/dev/null 2>&1 & w=$!; fi; \
        wait $p; s=$?; [ -z \"$w\" ] || kill $w 2>/dev/null; exit $s'";
    assert_eq!(scan, scan_cmd("alice/files/Fotos"));
    // The files folder holds other files too; `Fotos` only what was sent.
    let count_root = count_cmd(&[("", 6)]);
    let count_fotos = count_cmd(&[("Fotos", 6)]);
    shell
        .on("id -u", Output::ok("0\n"))
        .on(dir_mode.clone(), Output::ok(""))
        .on(dir_owner.clone(), Output::ok(""))
        .on(chown.clone(), Output::ok(""))
        .on(find.clone(), Output::ok(""))
        .on(count_root.clone(), Output::ok("6\n"))
        .on(count_fotos.clone(), Output::ok("5\n"))
        .on(scan, Output::ok(SCAN_FOTOS));

    let (sink, mut rx) = IntegrationSink::channel();
    let report = nc
        .after_transfer(&ctx(FILES, TransportCaps::RESUME), &changed, sink)
        .await
        .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert_eq!(
        shell.ran(),
        [
            "id -u",
            dir_mode.as_str(),
            dir_owner.as_str(),
            chown.as_str(),
            find.as_str(),
            count_root.as_str(),
            count_fotos.as_str(),
            scan
        ]
    );

    assert_eq!(report.updated, ["alice/files/Fotos"]);
    assert_eq!(report.items, 4, "from the summary table");
    assert_eq!(report.problems.len(), 1);
    let p = &report.problems[0];
    assert_eq!(p.code, PROBLEM_ENCODING);
    assert_eq!(p.path.as_deref(), Some("Fotos/Vera\u{303}o"));
    assert_eq!(p.fix, Some(FixAction::RepairDestination));

    let events = drain(&mut rx);
    assert_eq!(events[0], IntegrationEvent::Stage(STAGE_PERMISSIONS.into()));
    assert_eq!(events[1], IntegrationEvent::Stage(STAGE_SCAN.into()));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, IntegrationEvent::Problem(p) if p.code == PROBLEM_ENCODING))
    );
    let IntegrationEvent::Progress(last) = events.last().unwrap() else {
        panic!("last event is the final progress");
    };
    assert_eq!((last.done, last.total, last.items), (1, 1, 4));
}

#[tokio::test]
async fn transport_that_sets_owner_skips_permissions() {
    let (shell, _, nc) = setup();
    shell
        .on(count_cmd(&[("", 2)]), Output::ok("2\n"))
        .on(count_cmd(&[("Docs", 2)]), Output::ok("1\n"))
        .on(scan_cmd("alice/files/Docs"), Output::ok(""));
    let report = nc
        .after_transfer(
            &ctx(FILES, TransportCaps::CHOWN | TransportCaps::CHMOD),
            &paths(&["Docs/a.pdf"]),
            IntegrationSink::noop(),
        )
        .await
        .unwrap();
    assert_eq!(shell.ran().last(), Some(&scan_cmd("alice/files/Docs")));
    assert!(report.is_clean());
}

#[tokio::test]
async fn destination_subfolder_and_failed_scans() {
    let (shell, _, nc) = setup();
    let root = format!("{FILES}/Migração 2024");
    let changed = paths(&["Docs/IRS/2023.pdf", "Música/fado.flac"]);
    shell
        .on("id -u", Output::ok("0\n"))
        .on(
            format!(
                "find '{FILES}/Migração 2024' '{FILES}/Migração 2024/Docs' \
                 '{FILES}/Migração 2024/Docs/IRS' '{FILES}/Migração 2024/Música' \
                 -maxdepth 0 -type d -user 0 -exec chown -h 100033:100033 '{{}}' +"
            ),
            Output::ok(""),
        )
        .on(
            format!(
                "chown -h -- 100033:100033 '{FILES}/Migração 2024/Docs/IRS/2023.pdf' \
                 '{FILES}/Migração 2024/Música/fado.flac'"
            ),
            Output::ok(""),
        )
        .on(count_cmd(&[("Migração 2024", 3)]), Output::ok("3\n"))
        .on(
            count_cmd(&[("Migração 2024/Docs", 2), ("Migração 2024/Música", 2)]),
            Output::ok("2\n1\n"),
        )
        .on(count_cmd(&[("Migração 2024/Docs/IRS", 2)]), Output::ok("1\n"))
        .on(
            scan_cmd("alice/files/Migração 2024/Docs/IRS"),
            Output {
                status: 1,
                stdout: "Starting scan for user 1 out of 1 (alice)\n\
                         Another process is already scanning '/alice/files/Migração 2024/Docs/IRS'\n"
                    .into(),
                stderr: String::new(),
            },
        )
        .on(
            scan_cmd("alice/files/Migração 2024/Música"),
            Output::fail(255, "PHP Fatal error:  Allowed memory size exhausted"),
        );
    let report = nc
        .after_transfer(
            &ctx(&root, TransportCaps::CHMOD),
            &changed,
            IntegrationSink::noop(),
        )
        .await
        .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert_eq!(
        report.updated,
        [
            "alice/files/Migração 2024/Música",
            "alice/files/Migração 2024/Docs/IRS"
        ]
    );
    let codes: Vec<_> = report
        .problems
        .iter()
        .map(|p| (p.code.as_str(), p.retryable))
        .collect();
    assert_eq!(
        codes,
        [(PROBLEM_SCAN_FAILED, true), (PROBLEM_SCAN_ERROR, true)]
    );
    assert!(report.problems[0].message.contains("memory size"));
}

#[tokio::test]
async fn scans_only_sent_files_next_to_existing_ones() {
    let (shell, _, nc) = setup();
    let count = "sh -c 'while [ $# -gt 1 ]; do find \"$2\" -type f 2>/dev/null | head -n \"$1\" \
        | wc -l; shift 2; done' sh 2 /tank/nextcloud-data/alice/files";
    assert_eq!(count, count_cmd(&[("", 2)]));
    // 7 400 files already in the account: only the sent one is scanned.
    shell
        .on(count, Output::ok("2\n"))
        .on(scan_cmd("alice/files/cv.pdf"), Output::ok(""));
    let report = nc
        .after_transfer(
            &ctx(FILES, TransportCaps::CHOWN | TransportCaps::CHMOD),
            &paths(&["cv.pdf"]),
            IntegrationSink::noop(),
        )
        .await
        .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert_eq!(report.updated, ["alice/files/cv.pdf"]);
}

#[tokio::test]
async fn many_sent_files_beside_existing_ones_scan_shallow() {
    let (shell, _, nc) = setup();
    let changed: Vec<String> = (0..20).map(|i| format!("Docs/{i}.pdf")).collect();
    let changed: Vec<&str> = changed.iter().map(String::as_str).collect();
    let shallow = profile().occ_cmd_stop_on_eof(&[
        "files:scan",
        "-v",
        "--shallow",
        "--path=alice/files/Docs",
    ]);
    shell
        .on(count_cmd(&[("", 21)]), Output::ok("21\n"))
        .on(count_cmd(&[("Docs", 21)]), Output::ok("21\n"))
        .on(shallow.clone(), Output::ok(""));
    nc.after_transfer(
        &ctx(FILES, TransportCaps::CHOWN | TransportCaps::CHMOD),
        &paths(&changed),
        IntegrationSink::noop(),
    )
    .await
    .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert_eq!(shell.ran().last(), Some(&shallow));
}

#[tokio::test]
async fn failed_chown_stops_before_scanning() {
    let (shell, _, nc) = setup();
    shell.on(
        format!("chown -h -- 100033:100033 {FILES}/a.txt"),
        Output::fail(
            1,
            "chown: changing ownership of 'a.txt': Operation not permitted",
        ),
    );
    let err = nc
        .after_transfer(
            &ctx(FILES, TransportCaps::CHMOD),
            &paths(&["a.txt"]),
            IntegrationSink::noop(),
        )
        .await
        .unwrap_err();
    assert!(!err.retryable);
    assert!(err.message.contains("Operation not permitted"));
    assert_eq!(shell.ran().len(), 1);
}

#[tokio::test]
async fn nothing_changed_runs_nothing() {
    let (shell, connector, nc) = setup();
    let report = nc
        .after_transfer(
            &ctx(FILES, TransportCaps::empty()),
            &[],
            IntegrationSink::noop(),
        )
        .await
        .unwrap();
    assert!(report.is_clean() && report.updated.is_empty());
    assert!(shell.ran().is_empty() && connector.targets().is_empty());
}

#[tokio::test(start_paused = true)]
async fn slow_disk_shows_waiting_then_progress() {
    let (shell, _, nc) = setup();
    let nc = nc.with_options(DirectOptions {
        quiet_after: Duration::from_secs(15),
        ..DirectOptions::default()
    });
    let files: String = (0..150)
        .map(|i| format!("\tFile\t/alice/files/Video/clip{i}.mp4\n"))
        .collect();
    shell
        .on(count_cmd(&[("", 2)]), Output::ok("2\n"))
        .on(count_cmd(&[("Video", 2)]), Output::ok("1\n"));
    // Each line arrives only after a long silence.
    shell.on_slow(
        scan_cmd("alice/files/Video"),
        Output::ok(format!(
            "Starting scan for user 1 out of 1 (alice)\n{files}"
        )),
        Duration::from_secs(20),
    );
    let (sink, mut rx) = IntegrationSink::channel();
    let report = nc
        .after_transfer(
            &ctx(FILES, TransportCaps::CHOWN | TransportCaps::CHMOD),
            &paths(&["Video/clip0.mp4"]),
            sink,
        )
        .await
        .unwrap();
    assert_eq!(
        report.items, 150,
        "counted from File lines when there is no table"
    );
    let events = drain(&mut rx);
    assert!(events.contains(&IntegrationEvent::Waiting(WAITING_FOR_DISK.into())));
    let mid = events
        .iter()
        .any(|e| matches!(e, IntegrationEvent::Progress(p) if p.items == 100 && p.done == 0));
    assert!(mid, "progress every 100 files: {events:?}");
}

/// The SSH user `ssh_uid` and the sent `files`, with each parent folder's commands.
fn permission_cmds(ssh_uid: u32, dirs: &[&str], files: &[&str]) -> Vec<String> {
    let q = |v: &[&str]| -> String {
        v.iter()
            .map(|p| {
                filebridge_integration_nextcloud::quote::quote(&format!("{FILES}/{p}")).into_owned()
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut out = vec!["id -u".to_string()];
    if !dirs.is_empty() && ssh_uid != 100_033 {
        let d = q(dirs);
        out.push(format!(
            "find {d} -maxdepth 0 -type d -user {ssh_uid} -exec chmod 750 '{{}}' +"
        ));
        out.push(format!(
            "find {d} -maxdepth 0 -type d -user {ssh_uid} -exec chown -h 100033:100033 '{{}}' +"
        ));
    }
    let f = q(files);
    out.push(format!("chown -h -- 100033:100033 {f}"));
    out.push(format!(
        "find {f} -maxdepth 0 -type f -exec chmod 640 '{{}}' +"
    ));
    out
}

#[tokio::test]
async fn a_resumed_job_fixes_the_same_folders_and_nothing_else() {
    // Folders already fixed belong to 100033 now, so the server's `-user` test skips them: the
    // integration keeps no list, and a second run (a resumed job) sends the same commands.
    let (shell, _, nc) = setup();
    let cmds = permission_cmds(0, &["Fotos", "Fotos/2026"], &["Fotos/2026/a.jpg"]);
    for c in &cmds[1..] {
        shell.on(c.clone(), Output::ok(""));
    }
    shell.on("id -u", Output::ok("0\n"));
    shell
        .on(count_cmd(&[("", 2)]), Output::ok("1\n"))
        .on(scan_cmd("alice/files"), Output::ok(""));
    let changed = paths(&["Fotos/2026/a.jpg"]);
    let c = ctx(FILES, TransportCaps::RESUME);
    nc.after_transfer(&c, &changed, IntegrationSink::noop())
        .await
        .unwrap();
    let first = shell.ran();
    nc.after_transfer(&c, &changed, IntegrationSink::noop())
        .await
        .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert_eq!(shell.ran()[first.len()..], first[..]);
    assert_eq!(first[..cmds.len()], cmds[..]);
}

#[tokio::test]
async fn ssh_user_as_the_owner_leaves_folders_alone() {
    // Nothing tells the folders a send made from the old ones: all of them stay as they are.
    let (shell, _, nc) = setup();
    let cmds = permission_cmds(100_033, &["Docs"], &["Docs/a.pdf"]);
    assert_eq!(cmds.len(), 3);
    shell.on("id -u", Output::ok("100033\n"));
    for c in &cmds[1..] {
        shell.on(c.clone(), Output::ok(""));
    }
    shell
        .on(count_cmd(&[("", 2)]), Output::ok("1\n"))
        .on(scan_cmd("alice/files"), Output::ok(""));
    nc.after_transfer(
        &ctx(FILES, TransportCaps::RESUME),
        &paths(&["Docs/a.pdf"]),
        IntegrationSink::noop(),
    )
    .await
    .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert!(!shell.ran().iter().any(|c| c.contains("-type d")));
}

#[tokio::test]
async fn unreadable_ssh_uid_stops_before_any_change() {
    let (shell, _, nc) = setup();
    shell.on("id -u", Output::ok("root\n"));
    let err = nc
        .after_transfer(
            &ctx(FILES, TransportCaps::RESUME),
            &paths(&["Docs/a.pdf"]),
            IntegrationSink::noop(),
        )
        .await
        .unwrap_err();
    assert!(err.retryable, "{err}");
    assert_eq!(shell.ran(), ["id -u"]);
}

/// The same commands through a real `sh` on a temporary folder: real `find` and `chmod`, with
/// `id` answering a chosen uid, `chown` only logging (changing owners needs root) and `occ`
/// doing nothing.
#[cfg(unix)]
mod on_a_real_disk {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    use async_trait::async_trait;
    use filebridge_integration_nextcloud::RemoteShell;

    struct LocalShell {
        path: String,
        uid: u32,
        log: PathBuf,
    }

    #[async_trait]
    impl RemoteShell for LocalShell {
        async fn run(&self, cmd: &str) -> filebridge_integration_api::Result<Output> {
            let out = std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg(cmd)
                .env("PATH", &self.path)
                .env("FAKE_UID", self.uid.to_string())
                .env("CHOWN_LOG", &self.log)
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            Ok(Output {
                status: out.status.code().unwrap_or(-1),
                stdout: String::from_utf8_lossy(&out.stdout).into(),
                stderr: String::from_utf8_lossy(&out.stderr).into(),
            })
        }
    }

    struct LocalConnector(Arc<LocalShell>);

    #[async_trait]
    impl filebridge_integration_nextcloud::ShellConnector for LocalConnector {
        async fn connect(
            &self,
            _: &SshTarget,
        ) -> filebridge_integration_api::Result<Arc<dyn RemoteShell>> {
            Ok(self.0.clone())
        }
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o7777
    }

    fn set_mode(p: &Path, m: u32) {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
    }

    /// Runs the step over `files` (one existing folder `Fotos`, a new `Fotos/2026`) with `id -u`
    /// answering `uid`; returns the folders' modes and what `chown` was given.
    async fn run(uid_of: impl Fn(u32) -> u32) -> (u32, u32, u32, String) {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for (name, body) in [
            ("id", "echo \"$FAKE_UID\"\n"),
            ("chown", "printf '%s\\n' \"$*\" >> \"$CHOWN_LOG\"\n"),
            ("sudo", "exit 0\n"),
        ] {
            let p = bin.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
            set_mode(&p, 0o755);
        }
        let data = tmp.path().join("data");
        let files = data.join("alice/files");
        let fotos = files.join("Fotos");
        let new = fotos.join("2026");
        std::fs::create_dir_all(&new).unwrap();
        set_mode(&fotos, 0o775);
        set_mode(&new, 0o755);
        std::fs::write(new.join("a.jpg"), b"x").unwrap();
        set_mode(&new.join("a.jpg"), 0o644);
        let mine = std::fs::metadata(&fotos).unwrap().uid();

        let mut p = profile();
        p.runtime = Runtime::Host;
        p.run_as = RunAs::Sudo;
        p.data_dir = data.to_str().unwrap().into();
        p.host_data_dir = p.data_dir.clone();
        let mut c = ctx(files.to_str().unwrap(), TransportCaps::RESUME);
        c.profile.settings = serde_json::to_value(NextcloudSettings::Direct(Box::new(p))).unwrap();

        let log = tmp.path().join("chown.log");
        let shell = Arc::new(LocalShell {
            path: format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            uid: uid_of(mine),
            log: log.clone(),
        });
        let nc = Nextcloud::new(Arc::new(LocalConnector(shell)));
        nc.after_transfer(&c, &paths(&["Fotos/2026/a.jpg"]), IntegrationSink::noop())
            .await
            .unwrap();
        let chowned = std::fs::read_to_string(&log)
            .unwrap_or_default()
            .replace(&format!("{}/", files.display()), "");
        (mode(&fotos), mode(&new), mode(&new.join("a.jpg")), chowned)
    }

    #[tokio::test]
    async fn folders_owned_by_the_ssh_user_are_fixed() {
        let (fotos, new, file, chowned) = run(|mine| mine).await;
        assert_eq!((fotos, new, file), (0o750, 0o750, 0o640));
        assert_eq!(
            chowned,
            "-h 100033:100033 Fotos Fotos/2026\n-h -- 100033:100033 Fotos/2026/a.jpg\n"
        );
    }

    #[tokio::test]
    async fn folders_owned_by_someone_else_keep_owner_and_mode() {
        // The SSH user is not who owns these folders: they were there before the send.
        let (fotos, new, file, chowned) = run(|mine| mine + 1).await;
        assert_eq!((fotos, new, file), (0o775, 0o755, 0o640));
        assert_eq!(chowned, "-h -- 100033:100033 Fotos/2026/a.jpg\n");
    }
}

#[tokio::test]
async fn a_dedicated_user_hands_files_to_the_group_without_root() {
    // ADR 0024: it owns what it sends, so `chgrp` to the web server's group and group-writable
    // modes need no root; `occ` goes through the rule.
    let (shell, _, nc) = setup();
    let mut p = profile();
    p.ssh.user = "filebridge".into();
    p.runtime = Runtime::Host;
    p.data_dir = "/tank/nextcloud-data".into();
    p.run_as = RunAs::SudoRule;
    p.php = "/usr/bin/php".into();
    p.owner = Owner { uid: 33, gid: 33 };
    p.dir_mode = "770".into();
    p.file_mode = "660".into();
    p.access = filebridge_integration_nextcloud::Access::Dedicated;
    let mut c = ctx(FILES, TransportCaps::RESUME);
    c.profile.settings =
        serde_json::to_value(NextcloudSettings::Direct(Box::new(p.clone()))).unwrap();
    let cmds = [
        "id -u".to_string(),
        format!("find {FILES}/Fotos -maxdepth 0 -type d -user 1001 -exec chmod 770 '{{}}' +"),
        format!("find {FILES}/Fotos -maxdepth 0 -type d -user 1001 -exec chgrp -h 33 '{{}}' +"),
        format!("chgrp -h -- 33 {FILES}/Fotos/a.jpg"),
        format!("find {FILES}/Fotos/a.jpg -maxdepth 0 -type f -exec chmod 660 '{{}}' +"),
    ];
    shell.on("id -u", Output::ok("1001\n"));
    for cmd in &cmds[1..] {
        shell.on(cmd.clone(), Output::ok(""));
    }
    let scan = p.occ_cmd_stop_on_eof(&["files:scan", "-v", "--path=alice/files"]);
    assert!(scan.starts_with("sh -c "), "{scan}");
    shell
        .on(count_cmd(&[("", 2)]), Output::ok("1\n"))
        .on(scan, Output::ok(""));
    nc.after_transfer(&c, &paths(&["Fotos/a.jpg"]), IntegrationSink::noop())
        .await
        .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert_eq!(shell.ran()[..cmds.len()], cmds[..]);
    assert!(shell.ran().iter().all(|c| !c.contains("chown")));
}
