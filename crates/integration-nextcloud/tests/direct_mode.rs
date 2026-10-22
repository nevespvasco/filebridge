//! Direct-mode `preflight` and `after_transfer` through the `Integration` trait, over `FakeShell`.

use std::sync::Arc;
use std::time::Duration;

use filebridge_integration_api::{
    CheckStatus, Destination, FixAction, Integration, IntegrationEvent, IntegrationSink,
    JobContext, ProfileView, RemotePath, TransportCaps,
};
use filebridge_integration_nextcloud::direct::{
    PROBLEM_ENCODING, PROBLEM_SCAN_ERROR, PROBLEM_SCAN_FAILED, STAGE_PERMISSIONS, STAGE_SCAN,
    WAITING_FOR_DISK,
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
            settings: serde_json::to_value(NextcloudSettings::Direct(profile())).unwrap(),
        },
        caps,
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
        .on(format!("df -Pk {FILES}"), Output::ok(DF));
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
    // Exact command lines: paths with spaces, quotes and accents are single-quoted.
    let targets = "/tank/nextcloud-data/alice/files/Fotos \
        /tank/nextcloud-data/alice/files/Fotos/2024 \
        '/tank/nextcloud-data/alice/files/Fotos/2024/IMG 0002 (cópia).jpg' \
        /tank/nextcloud-data/alice/files/Fotos/2024/IMG_0001.jpg \
        '/tank/nextcloud-data/alice/files/Fotos/Viagem a Óbidos' \
        '/tank/nextcloud-data/alice/files/Fotos/Viagem a Óbidos/O'\\''Brien \"festa\".jpg' \
        '/tank/nextcloud-data/alice/files/Fotos/Viagem a Óbidos/mapa.pdf' \
        /tank/nextcloud-data/alice/files/Fotos/capa.jpg";
    let chown = format!("chown -h -- 100033:100033 {targets}");
    let find = format!(
        "find {targets} -maxdepth 0 -type d -exec chmod 750 '{{}}' + -o -type f -exec chmod 640 '{{}}' +"
    );
    // `occ` runs in the background; a watcher on the SSH channel (stdin) stops it on EOF.
    let scan = "pct exec 105 -- su -s /bin/bash www-data -c 'exec 3<&0; \
        php /var/www/nextcloud/occ files:scan -v --path=alice/files/Fotos </dev/null & p=$!; w=; \
        if [ -p /dev/stdin ] || [ -S /dev/stdin ]; then \
        (while read -r l; do :; done <&3; kill -TERM $p) >/dev/null 2>&1 & w=$!; fi; \
        wait $p; s=$?; [ -z \"$w\" ] || kill $w 2>/dev/null; exit $s'";
    assert_eq!(scan, scan_cmd("alice/files/Fotos"));
    shell
        .on(chown.clone(), Output::ok(""))
        .on(find.clone(), Output::ok(""))
        .on(scan, Output::ok(SCAN_FOTOS));

    let (sink, mut rx) = IntegrationSink::channel();
    let report = nc
        .after_transfer(&ctx(FILES, TransportCaps::RESUME), &changed, sink)
        .await
        .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert_eq!(shell.ran(), [chown.as_str(), find.as_str(), scan]);

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
    shell.on(scan_cmd("alice/files/Docs"), Output::ok(""));
    let report = nc
        .after_transfer(
            &ctx(FILES, TransportCaps::CHOWN | TransportCaps::CHMOD),
            &paths(&["Docs/a.pdf"]),
            IntegrationSink::noop(),
        )
        .await
        .unwrap();
    assert_eq!(shell.ran(), [scan_cmd("alice/files/Docs")]);
    assert!(report.is_clean());
}

#[tokio::test]
async fn destination_subfolder_and_failed_scans() {
    let (shell, _, nc) = setup();
    let root = format!("{FILES}/Migração 2024");
    let changed = paths(&["Docs/IRS/2023.pdf", "Música/fado.flac"]);
    shell
        .on(
            format!(
                "chown -h -- 100033:100033 '{FILES}/Migração 2024' '{FILES}/Migração 2024/Docs' \
                 '{FILES}/Migração 2024/Docs/IRS' '{FILES}/Migração 2024/Docs/IRS/2023.pdf' \
                 '{FILES}/Migração 2024/Música' '{FILES}/Migração 2024/Música/fado.flac'"
            ),
            Output::ok(""),
        )
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
            "alice/files/Migração 2024/Docs/IRS",
            "alice/files/Migração 2024/Música"
        ]
    );
    let codes: Vec<_> = report
        .problems
        .iter()
        .map(|p| (p.code.as_str(), p.retryable))
        .collect();
    assert_eq!(
        codes,
        [(PROBLEM_SCAN_ERROR, true), (PROBLEM_SCAN_FAILED, true)]
    );
    assert!(report.problems[1].message.contains("memory size"));
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
