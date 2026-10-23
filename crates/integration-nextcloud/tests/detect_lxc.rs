//! `LxcDetector` end to end over `FakeShell`, with captured-style Proxmox and Nextcloud output.

use std::sync::Arc;

use filebridge_integration_api::{CheckStatus, Integration};
use filebridge_integration_nextcloud::fake::{FakeConnector, FakeShell};
use filebridge_integration_nextcloud::{
    DetectInput, Detector, LxcDetector, Nextcloud, NextcloudDirectProfile, Output, Owner, RunAs,
    Runtime, SetupError, SetupRequest, SshTarget,
};
use filebridge_transport_api::Backend;

const PCT_LIST: &str = include_str!("fixtures/pct_list.txt");
const PCT_CONFIG_105: &str = include_str!("fixtures/pct_config_105.txt");
const PCT_CONFIG_IDMAP: &str = include_str!("fixtures/pct_config_custom_idmap.txt");
const CONFIG_PHP: &str = include_str!("fixtures/config.php");
const OCC_STATUS: &str = include_str!("fixtures/occ_status.json");

const LS_INSTALL: &str = "pct exec 105 -- ls -d /var/www/nextcloud/config/config.php \
    /var/www/html/nextcloud/config/config.php /var/www/html/config/config.php \
    /usr/share/webapps/nextcloud/config/config.php /srv/nextcloud/config/config.php \
    /opt/nextcloud/config/config.php";
const GREP_DATADIR: &str =
    "pct exec 105 -- grep -F datadirectory /var/www/nextcloud/config/config.php";
const OCC_STATUS_SU: &str = "pct exec 105 -- su -s /bin/bash www-data -c \
    'php /var/www/nextcloud/occ status --output=json'";

fn ssh() -> SshTarget {
    SshTarget {
        host: "pve.lan".into(),
        port: 22,
        user: "root".into(),
        key_file: None,
        known_hosts_file: None,
        key: None,
    }
}

fn input() -> DetectInput {
    DetectInput {
        ssh: ssh(),
        nextcloud_user: "alice".into(),
        ctid: None,
        install_dir: None,
    }
}

/// What `grep -F datadirectory` prints for `config_php`.
fn grep(config_php: &str) -> Output {
    let lines: String = config_php
        .lines()
        .filter(|l| l.contains("datadirectory"))
        .map(|l| format!("{l}\n"))
        .collect();
    Output::ok(lines)
}

/// The author's setup: root on the Proxmox node, CT 105, data on a bind mount, no sudo inside.
fn authors_setup() -> FakeShell {
    let shell = FakeShell::new();
    shell
        .on("pct list", Output::ok(PCT_LIST))
        .on("pct config 105", Output::ok(PCT_CONFIG_105))
        .on(
            LS_INSTALL,
            Output {
                status: 2,
                stdout: "/var/www/nextcloud/config/config.php\n".into(),
                stderr: "ls: cannot access '/var/www/html/nextcloud/config/config.php': No such file or directory\n".into(),
            },
        )
        .on(GREP_DATADIR, grep(CONFIG_PHP))
        .on("pct exec 105 -- test -d /mnt/ncdata", Output::ok(""))
        .on("test -d /tank/nextcloud-data", Output::ok(""))
        .on("test -d /tank/nextcloud-data/alice/files", Output::ok(""))
        .on("test -w /tank/nextcloud-data/alice/files", Output::ok(""))
        .on("pct exec 105 -- test -f /var/www/nextcloud/occ", Output::ok(""))
        .on(
            "pct exec 105 -- stat -c %U /var/www/nextcloud/config/config.php",
            Output::ok("www-data\n"),
        )
        .on("pct exec 105 -- sh -c 'command -v sudo'", Output::fail(1, ""))
        .on(OCC_STATUS_SU, Output::ok(OCC_STATUS))
        .on(
            "pct exec 105 -- id www-data",
            Output::ok("uid=33(www-data) gid=33(www-data) groups=33(www-data)\n"),
        )
        .on("stat -c %u:%g /tank/nextcloud-data", Output::ok("100033:100033\n"));
    shell
}

fn statuses(d: &filebridge_integration_nextcloud::Detection) -> Vec<(String, CheckStatus)> {
    d.steps
        .iter()
        .map(|s| (s.check.id.clone(), s.check.status))
        .collect()
}

#[tokio::test]
async fn detects_the_authors_lxc() {
    let shell = authors_setup();
    let d = LxcDetector.detect(&shell, &input()).await;
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert_eq!(
        statuses(&d),
        [
            ("pct-list".into(), CheckStatus::Ok),
            ("pct-config".into(), CheckStatus::Ok),
            ("datadirectory".into(), CheckStatus::Ok),
            ("data-path".into(), CheckStatus::Ok),
            ("writable".into(), CheckStatus::Ok),
            ("occ".into(), CheckStatus::Ok),
            ("run-as".into(), CheckStatus::Ok),
            ("owner".into(), CheckStatus::Ok),
        ]
    );
    assert_eq!(
        d.profile,
        Some(NextcloudDirectProfile {
            ssh: ssh(),
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
                gid: 100_033
            },
            dir_mode: "750".into(),
            file_mode: "640".into(),
        })
    );
    assert_eq!(d.containers.len(), 4);
    // Each step lists what it ran, for the Details panel.
    let run_as = &d.steps[6];
    assert_eq!(
        run_as
            .commands
            .iter()
            .map(|c| c.command.as_str())
            .collect::<Vec<_>>(),
        ["pct exec 105 -- sh -c 'command -v sudo'", OCC_STATUS_SU]
    );
    assert_eq!(run_as.commands[0].status, 1);
    assert!(run_as.check.message.contains("su (no sudo"));
}

#[tokio::test]
async fn never_reads_secrets_from_config_php() {
    let shell = authors_setup();
    let d = LxcDetector.detect(&shell, &input()).await;
    assert!(shell.ran().iter().all(|c| !c.contains("cat ")));
    let json = serde_json::to_string(&d).unwrap();
    assert!(
        !json.contains("FIXTURE-NOT-A-REAL"),
        "secrets leaked into the detection"
    );
}

#[tokio::test]
async fn data_on_the_container_disk_is_explained() {
    let shell = authors_setup();
    let old = CONFIG_PHP.replace(
        "'datadirectory' => '/mnt/ncdata'",
        "'datadirectory' => '/var/www/nextcloud/data'",
    );
    shell.on(GREP_DATADIR, grep(&old));
    let d = LxcDetector.detect(&shell, &input()).await;
    assert!(d.profile.is_none());
    let last = d.steps.last().unwrap();
    assert_eq!(last.check.id, "datadirectory");
    assert_eq!(last.check.status, CheckStatus::Fail);
    assert!(
        last.check.message.contains("container's own disk"),
        "{}",
        last.check.message
    );
}

#[tokio::test]
async fn data_on_a_storage_volume_is_resolved_with_pvesm() {
    let shell = authors_setup();
    let vol = CONFIG_PHP.replace("'/mnt/ncdata'", "'/srv/extra/nc data'");
    shell
        .on(GREP_DATADIR, grep(&vol))
        .on(
            "pvesm path local-zfs:subvol-105-disk-1",
            Output::ok("/rpool/data/subvol-105-disk-1\n"),
        )
        .on(
            "pct exec 105 -- test -d '/srv/extra/nc data'",
            Output::ok(""),
        )
        .on(
            "test -d '/rpool/data/subvol-105-disk-1/nc data'",
            Output::ok(""),
        )
        .on(
            "test -d '/rpool/data/subvol-105-disk-1/nc data/alice/files'",
            Output::ok(""),
        )
        .on(
            "test -w '/rpool/data/subvol-105-disk-1/nc data/alice/files'",
            Output::ok(""),
        )
        .on(
            "stat -c %u:%g '/rpool/data/subvol-105-disk-1/nc data'",
            Output::ok("100033:100033\n"),
        );
    let d = LxcDetector.detect(&shell, &input()).await;
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    let p = d.profile.expect("detected");
    assert_eq!(p.data_dir, "/srv/extra/nc data");
    assert_eq!(p.host_data_dir, "/rpool/data/subvol-105-disk-1/nc data");
}

#[tokio::test]
async fn dedicated_user_with_sudo_and_custom_idmap() {
    let shell = FakeShell::new();
    let list =
        "VMID       Status     Lock         Name\n130        running                 nc-photos\n";
    shell
        .on("pct list", Output::fail(2, "Unable to load access control list: Permission denied\n"))
        .on("sudo -n pct list", Output::ok(list))
        .on("sudo -n pct config 130", Output::ok(PCT_CONFIG_IDMAP))
        .on(
            "sudo -n pct exec 130 -- grep -F datadirectory /var/www/html/config/config.php",
            Output::ok("  'datadirectory' => '/srv/nextcloud/data',\n"),
        )
        .on("sudo -n pct exec 130 -- test -d /srv/nextcloud/data", Output::ok(""))
        .on("sudo -n test -d /mnt/pve/hdd/nextcloud", Output::ok(""))
        .on("sudo -n test -d /mnt/pve/hdd/nextcloud/ana/files", Output::ok(""))
        // The data folder belongs to the `filebridge` user (the custom idmap), so it writes there.
        .on("test -w /mnt/pve/hdd/nextcloud/ana/files", Output::ok(""))
        .on("sudo -n pct exec 130 -- test -f /var/www/html/occ", Output::ok(""))
        .on(
            "sudo -n pct exec 130 -- stat -c %U /var/www/html/config/config.php",
            Output::ok("www-data\n"),
        )
        .on(
            "sudo -n pct exec 130 -- sh -c 'command -v sudo'",
            Output::ok("/usr/bin/sudo\n"),
        )
        .on(
            "sudo -n pct exec 130 -- sudo -n -u www-data php /var/www/html/occ status --output=json",
            Output::ok(OCC_STATUS.replace("\"maintenance\":false", "\"maintenance\":true")),
        )
        .on(
            "sudo -n pct exec 130 -- id www-data",
            Output::ok("uid=33(www-data) gid=33(www-data) groups=33(www-data)\n"),
        )
        .on("sudo -n stat -c %u:%g /mnt/pve/hdd/nextcloud", Output::ok("1005:1005\n"));
    let input = DetectInput {
        ssh: SshTarget {
            user: "filebridge".into(),
            ..ssh()
        },
        nextcloud_user: "ana".into(),
        ctid: Some(130),
        install_dir: Some("/var/www/html/".into()),
    };
    let d = LxcDetector.detect(&shell, &input).await;
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    let p = d.profile.expect("detected");
    assert!(p.host_sudo);
    assert_eq!(p.run_as, RunAs::Sudo);
    assert_eq!(
        p.owner,
        Owner {
            uid: 1005,
            gid: 1005
        }
    );
    assert_eq!(p.host_data_dir, "/mnt/pve/hdd/nextcloud");
    // Maintenance mode is a warning at detection time (preflight fails on it).
    let run_as = d.steps.iter().find(|s| s.check.id == "run-as").unwrap();
    assert_eq!(run_as.check.status, CheckStatus::Warn);
    // The failed plain attempt is shown before the sudo one.
    assert_eq!(d.steps[0].commands.len(), 2);
}

#[tokio::test]
async fn ambiguous_containers_ask_the_user() {
    let shell = FakeShell::new();
    shell.on(
        "pct list",
        Output::ok("VMID Status Lock Name\n105 running  nextcloud\n106 running  nextcloud-test\n"),
    );
    let d = LxcDetector.detect(&shell, &input()).await;
    assert!(d.profile.is_none());
    assert_eq!(d.containers.len(), 2);
    assert_eq!(statuses(&d), [("pct-list".into(), CheckStatus::Fail)]);
}

#[tokio::test]
async fn not_a_proxmox_host_hands_over_to_the_next_detector() {
    let shell = FakeShell::new();
    shell
        .on("pct list", Output::fail(127, "sh: 1: pct: not found\n"))
        .on("test -x /usr/sbin/pct", Output::fail(1, ""));
    let d = LxcDetector.detect(&shell, &input()).await;
    assert!(!d.applies);
    assert!(d.profile.is_none());
    assert_eq!(statuses(&d), [("pct-list".into(), CheckStatus::Ok)]);
    assert!(d.steps[0].check.message.contains("not a Proxmox node"));
    assert_eq!(
        shell.ran(),
        ["pct list", "test -x /usr/sbin/pct"],
        "no sudo retry when pct is missing"
    );
}

#[tokio::test]
async fn a_chosen_container_without_pct_fails() {
    let shell = FakeShell::new();
    shell
        .on("pct list", Output::fail(127, "sh: 1: pct: not found\n"))
        .on("test -x /usr/sbin/pct", Output::fail(1, ""));
    let input = DetectInput {
        ctid: Some(105),
        ..input()
    };
    let d = LxcDetector.detect(&shell, &input).await;
    assert!(d.applies, "the user asked for a container");
    assert_eq!(statuses(&d), [("pct-list".into(), CheckStatus::Fail)]);
    assert!(d.steps[0].check.message.contains("Proxmox node"));
}

#[tokio::test]
async fn pct_outside_a_non_root_path_is_retried_with_sudo() {
    // Debian gives non-root users no /usr/sbin in PATH; sudo's secure_path has it.
    let shell = FakeShell::new();
    shell
        .on("pct list", Output::fail(127, "sh: 1: pct: not found\n"))
        .on("test -x /usr/sbin/pct", Output::ok(""))
        .on("sudo -n pct list", Output::ok(PCT_LIST));
    let d = LxcDetector.detect(&shell, &input()).await;
    assert!(d.applies);
    assert_eq!(d.steps[0].check.id, "pct-list");
    assert_eq!(d.steps[0].check.status, CheckStatus::Ok);
    assert_eq!(
        shell.ran()[..3],
        ["pct list", "test -x /usr/sbin/pct", "sudo -n pct list"]
    );
    assert_eq!(shell.ran()[3], "sudo -n pct config 105");
}

#[tokio::test]
async fn lost_connection_fails_the_step() {
    let shell = FakeShell::unreachable();
    let d = LxcDetector.detect(&shell, &input()).await;
    assert!(d.profile.is_none());
    assert_eq!(d.steps.len(), 1);
    assert_eq!(d.steps[0].commands[0].status, -1);
}

fn setup_request(folder: &str) -> SetupRequest {
    SetupRequest {
        ssh: ssh(),
        nextcloud_user: "alice".into(),
        folder: folder.into(),
        ctid: None,
        install_dir: None,
        modes: None,
    }
}

#[tokio::test]
async fn the_wizard_builds_a_profile_that_passes_its_own_check() {
    let nextcloud = Nextcloud::new(Arc::new(FakeConnector::new(Arc::new(authors_setup()))));
    let setup = nextcloud
        .set_up(&setup_request("/Backup/2024/"), "some-transport")
        .await
        .unwrap();
    let parts = setup.profile.expect("every step passed");
    assert_eq!(parts.destination.transport, "some-transport");
    assert_eq!(
        parts.destination.root,
        "/tank/nextcloud-data/alice/files/Backup/2024"
    );
    assert!(matches!(
        &parts.destination.backend,
        Backend::Sftp { host, port: Some(22), user: Some(u), .. } if host == "pve.lan" && u == "root"
    ));
    nextcloud
        .check_profile(&parts.destination, &parts.settings)
        .unwrap();

    // Files sent to another host than the one Nextcloud runs on.
    let mut elsewhere = parts.destination.clone();
    if let Backend::Sftp { host, .. } = &mut elsewhere.backend {
        *host = "nas.lan".into();
    }
    assert!(
        nextcloud
            .check_profile(&elsewhere, &parts.settings)
            .is_err()
    );
    // A root outside the user's files folder.
    let outside = filebridge_transport_api::Destination {
        root: "/tank/nextcloud-data/someone-else/files".into(),
        ..parts.destination.clone()
    };
    assert!(nextcloud.check_profile(&outside, &parts.settings).is_err());
    // Not even Nextcloud settings.
    assert!(
        nextcloud
            .check_profile(&parts.destination, &serde_json::json!({ "mode": "webdav" }))
            .is_err()
    );
}

#[tokio::test]
async fn the_wizard_refuses_bad_modes_and_reports_a_failed_connection() {
    let nextcloud = Nextcloud::new(Arc::new(FakeConnector::new(Arc::new(authors_setup()))));
    let req = SetupRequest {
        modes: Some(("999".into(), "640".into())),
        ..setup_request("")
    };
    assert!(matches!(
        nextcloud.set_up(&req, "t").await,
        Err(SetupError::Invalid(_))
    ));

    let offline = Nextcloud::new(Arc::new(FakeConnector::refusing()));
    assert!(matches!(
        offline.set_up(&setup_request(""), "t").await,
        Err(SetupError::Connect(_))
    ));
}

#[tokio::test]
async fn a_failed_detection_has_no_profile() {
    let shell = Arc::new(FakeShell::new());
    shell.on(
        "pct list",
        Output::fail(2, "Unable to load access control list: Permission denied\n"),
    );
    let nextcloud = Nextcloud::new(Arc::new(FakeConnector::new(shell.clone())));
    let setup = nextcloud.set_up(&setup_request(""), "t").await.unwrap();
    assert!(setup.profile.is_none());
    assert_eq!(setup.detection.detector, LxcDetector::ID);
    assert_eq!(setup.detection.steps.len(), 1);
    assert_eq!(setup.detection.steps[0].check.status, CheckStatus::Fail);
    // A Proxmox node whose detection failed is not searched for Nextcloud on the node itself.
    assert_eq!(shell.ran(), ["pct list", "sudo -n pct list"]);
}
