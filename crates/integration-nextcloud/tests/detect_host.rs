//! `HostDetector` end to end over `FakeShell`: Nextcloud on the SSH host itself (its LXC or VM
//! reached directly, say over Tailscale, or a bare-metal server), and the wizard's fallback to
//! it when the host has no `pct`.

use std::sync::Arc;

use filebridge_integration_api::{
    CheckStatus, Destination, Integration, JobContext, ProfileView, TransportCaps,
};
use filebridge_integration_nextcloud::fake::{FakeConnector, FakeShell};
use filebridge_integration_nextcloud::{
    DetectInput, Detection, Detector, HostDetector, LxcDetector, Nextcloud, NextcloudDirectProfile,
    NextcloudSettings, Output, Owner, RunAs, Runtime, SetupRequest, SshTarget,
};
use filebridge_transport_api::Backend;

const CONFIG_PHP: &str = include_str!("fixtures/config_host.php");
const OCC_STATUS: &str = include_str!("fixtures/occ_status.json");
const DF: &str = include_str!("fixtures/df.txt");

const LS_INSTALL: &str = "ls -d /var/www/nextcloud/config/config.php \
    /var/www/html/nextcloud/config/config.php /var/www/html/config/config.php \
    /usr/share/webapps/nextcloud/config/config.php /srv/nextcloud/config/config.php \
    /opt/nextcloud/config/config.php";
const FILES: &str = "/mnt/ncdata/demo/files";

fn ssh(user: &str) -> SshTarget {
    SshTarget {
        host: "192.0.2.60".into(),
        port: 22,
        user: user.into(),
        key_file: None,
        known_hosts_file: None,
        key: None,
    }
}

fn input(user: &str) -> DetectInput {
    DetectInput {
        ssh: ssh(user),
        nextcloud_user: "demo".into(),
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
    if lines.is_empty() {
        Output::fail(1, "")
    } else {
        Output::ok(lines)
    }
}

/// No `pct` on the host (it is the Nextcloud machine, not the Proxmox node).
fn no_pct(shell: &FakeShell) {
    shell
        .on(
            "pct list",
            Output::fail(127, "bash: line 1: pct: command not found\n"),
        )
        .on("test -x /usr/sbin/pct", Output::fail(1, ""));
}

/// The reporter's setup: root over Tailscale straight into the Nextcloud LXC, which (like most
/// Debian containers) has no sudo, so `occ` runs through `su`.
fn root_without_sudo() -> FakeShell {
    let shell = FakeShell::new();
    no_pct(&shell);
    shell
        .on("id -u", Output::ok("0\n"))
        .on(
            LS_INSTALL,
            Output {
                status: 2,
                stdout: "/var/www/nextcloud/config/config.php\n".into(),
                stderr: "ls: cannot access '/var/www/html/config/config.php': No such file or directory\n".into(),
            },
        )
        .on(
            "grep -F datadirectory /var/www/nextcloud/config/config.php",
            grep(CONFIG_PHP),
        )
        .on("test -d /mnt/ncdata", Output::ok(""))
        .on(format!("test -d {FILES}"), Output::ok(""))
        .on(format!("test -w {FILES}"), Output::ok(""))
        .on("test -f /var/www/nextcloud/occ", Output::ok(""))
        .on(
            "stat -c %U /var/www/nextcloud/config/config.php",
            Output::ok("www-data\n"),
        )
        .on("sh -c 'command -v sudo'", Output::fail(1, ""))
        .on(
            "su -s /bin/bash www-data -c 'php /var/www/nextcloud/occ status --output=json'",
            Output::ok(OCC_STATUS),
        )
        .on(
            "id www-data",
            Output::ok("uid=33(www-data) gid=33(www-data) groups=33(www-data)\n"),
        )
        .on("stat -c %u:%g /mnt/ncdata", Output::ok("33:33\n"));
    shell
}

/// A VM or bare-metal server: a non-root SSH user with passwordless sudo, Nextcloud under
/// /var/www/html served by `nginx`. The user can write to the files folder itself (say, through
/// an ACL), since SFTP writes as that user.
fn sudo_user_on_a_server() -> FakeShell {
    let shell = FakeShell::new();
    no_pct(&shell);
    shell
        .on("id -u", Output::ok("1000\n"))
        .on("sudo -n true", Output::ok(""))
        .on(
            format!("sudo -n {LS_INSTALL}"),
            Output {
                status: 2,
                stdout: "/var/www/html/config/config.php\n".into(),
                stderr: String::new(),
            },
        )
        .on(
            "sudo -n grep -F datadirectory /var/www/html/config/config.php",
            Output::ok("  'datadirectory' => '/srv/nextcloud/data',\n"),
        )
        .on("sudo -n test -d /srv/nextcloud/data", Output::ok(""))
        .on(
            "sudo -n test -d /srv/nextcloud/data/ana/files",
            Output::ok(""),
        )
        .on("test -w /srv/nextcloud/data/ana/files", Output::ok(""))
        .on("sudo -n test -f /var/www/html/occ", Output::ok(""))
        .on(
            "sudo -n stat -c %U /var/www/html/config/config.php",
            Output::ok("nginx\n"),
        )
        .on(
            "sudo -n sh -c 'command -v sudo'",
            Output::ok("/usr/bin/sudo\n"),
        )
        .on(
            "sudo -n sudo -n -u nginx php /var/www/html/occ status --output=json",
            Output::ok(OCC_STATUS),
        )
        .on(
            "sudo -n id nginx",
            Output::ok("uid=998(nginx) gid=996(nginx) groups=996(nginx)\n"),
        )
        .on(
            "sudo -n stat -c %u:%g /srv/nextcloud/data",
            Output::ok("998:996\n"),
        );
    shell
}

fn statuses(d: &Detection) -> Vec<(String, CheckStatus)> {
    d.steps
        .iter()
        .map(|s| (s.check.id.clone(), s.check.status))
        .collect()
}

fn ok(ids: &[&str]) -> Vec<(String, CheckStatus)> {
    ids.iter()
        .map(|i| (i.to_string(), CheckStatus::Ok))
        .collect()
}

#[tokio::test]
async fn root_on_the_nextcloud_lxc_runs_occ_through_su() {
    let shell = root_without_sudo();
    let d = HostDetector.detect(&shell, &input("root")).await;
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert_eq!(d.detector, HostDetector::ID);
    assert!(d.applies);
    assert_eq!(
        statuses(&d),
        ok(&[
            "host",
            "datadirectory",
            "data-path",
            "writable",
            "occ",
            "run-as",
            "owner"
        ])
    );
    assert_eq!(
        d.profile,
        Some(NextcloudDirectProfile {
            ssh: ssh("root"),
            host_sudo: false,
            runtime: Runtime::Host,
            install_dir: "/var/www/nextcloud".into(),
            php: "php".into(),
            run_as: RunAs::Su,
            web_user: "www-data".into(),
            data_dir: "/mnt/ncdata".into(),
            host_data_dir: "/mnt/ncdata".into(),
            user: "demo".into(),
            owner: Owner { uid: 33, gid: 33 },
            dir_mode: "750".into(),
            file_mode: "640".into(),
        })
    );
    let run_as = d.steps.iter().find(|s| s.check.id == "run-as").unwrap();
    assert!(
        run_as
            .check
            .message
            .contains("su (no sudo on this machine)"),
        "{}",
        run_as.check.message
    );
    assert!(d.steps[0].check.message.contains("as root"));
    // Nothing goes through `pct exec`.
    assert!(shell.ran().iter().all(|c| !c.contains("pct exec")));
}

#[tokio::test]
async fn a_sudo_user_on_a_server_with_its_own_web_user() {
    let shell = sudo_user_on_a_server();
    let input = DetectInput {
        nextcloud_user: "ana".into(),
        ..input("deploy")
    };
    let d = HostDetector.detect(&shell, &input).await;
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    let p = d.profile.expect("detected");
    assert!(p.host_sudo);
    assert_eq!(p.run_as, RunAs::Sudo);
    assert_eq!(p.install_dir, "/var/www/html");
    assert_eq!(p.web_user, "nginx");
    assert_eq!(p.data_dir, "/srv/nextcloud/data");
    assert_eq!(p.host_data_dir, p.data_dir);
    // The web user's own ids: no idmap offset outside a Proxmox container view.
    assert_eq!(p.owner, Owner { uid: 998, gid: 996 });
    assert!(d.steps[0].check.message.contains("sudo -n"));
    assert_eq!(d.steps[0].commands.len(), 2, "id -u, then sudo -n true");
}

#[tokio::test]
async fn owner_mismatch_is_a_warning_not_an_idmap_shift() {
    let shell = root_without_sudo();
    shell.on("stat -c %u:%g /mnt/ncdata", Output::ok("100033:100033\n"));
    let d = HostDetector.detect(&shell, &input("root")).await;
    let p = d.profile.expect("a warning still saves");
    assert_eq!(p.owner, Owner { uid: 33, gid: 33 });
    let owner = d.steps.last().unwrap();
    assert_eq!(owner.check.id, "owner");
    assert_eq!(owner.check.status, CheckStatus::Warn);
    assert!(owner.check.message.contains("100033:100033"));
}

#[tokio::test]
async fn default_data_folder_when_config_php_has_none() {
    let shell = root_without_sudo();
    let none: String = CONFIG_PHP
        .lines()
        .filter(|l| !l.contains("datadirectory"))
        .collect::<Vec<_>>()
        .join("\n");
    shell
        .on(
            "grep -F datadirectory /var/www/nextcloud/config/config.php",
            grep(&none),
        )
        .on("test -d /var/www/nextcloud/data", Output::ok(""))
        .on("test -d /var/www/nextcloud/data/demo/files", Output::ok(""))
        .on("test -w /var/www/nextcloud/data/demo/files", Output::ok(""))
        .on(
            "stat -c %u:%g /var/www/nextcloud/data",
            Output::ok("33:33\n"),
        );
    let d = HostDetector.detect(&shell, &input("root")).await;
    let p = d.profile.expect("detected");
    assert_eq!(p.data_dir, "/var/www/nextcloud/data");
    let step = d
        .steps
        .iter()
        .find(|s| s.check.id == "datadirectory")
        .unwrap();
    assert!(step.check.message.contains("default"));
}

#[tokio::test]
async fn a_sudo_user_who_cannot_write_the_files_folder_is_told_to_use_root() {
    let shell = sudo_user_on_a_server();
    shell.on("test -w /srv/nextcloud/data/ana/files", Output::fail(1, ""));
    let input = DetectInput {
        nextcloud_user: "ana".into(),
        ..input("deploy")
    };
    let d = HostDetector.detect(&shell, &input).await;
    assert!(d.profile.is_none());
    let last = d.steps.last().unwrap();
    assert_eq!(last.check.id, "writable");
    assert_eq!(last.check.status, CheckStatus::Fail);
    assert!(
        last.check.message.contains("deploy cannot write"),
        "{}",
        last.check.message
    );
    assert!(last.check.message.contains("log in as root"));
    // Checked as the SSH user itself: sudo would hide the problem.
    assert_eq!(
        last.commands[0].command,
        "test -w /srv/nextcloud/data/ana/files"
    );
}

#[tokio::test]
async fn a_user_without_root_or_sudo_is_told_why() {
    let shell = FakeShell::new();
    shell.on("id -u", Output::ok("1000\n")).on(
        "sudo -n true",
        Output::fail(1, "sudo: a password is required\n"),
    );
    let d = HostDetector.detect(&shell, &input("alice")).await;
    assert!(d.profile.is_none());
    assert_eq!(statuses(&d), [("host".into(), CheckStatus::Fail)]);
    assert!(d.steps[0].check.message.contains("needs root"));
    assert!(d.steps[0].check.message.contains("password is required"));
}

#[tokio::test]
async fn no_nextcloud_install_found() {
    let shell = FakeShell::new();
    shell
        .on("id -u", Output::ok("0\n"))
        .on(LS_INSTALL, Output::fail(2, "ls: cannot access …\n"));
    let d = HostDetector.detect(&shell, &input("root")).await;
    assert!(d.profile.is_none());
    let last = d.steps.last().unwrap();
    assert_eq!(last.check.id, "datadirectory");
    assert!(
        last.check.message.contains("on this machine"),
        "{}",
        last.check.message
    );
}

#[tokio::test]
async fn never_reads_secrets_from_config_php() {
    let shell = root_without_sudo();
    let d = HostDetector.detect(&shell, &input("root")).await;
    assert!(shell.ran().iter().all(|c| !c.contains("cat ")));
    let json = serde_json::to_string(&d).unwrap();
    assert!(!json.contains("FIXTURE-NOT-A-REAL"));
}

fn setup_request(folder: &str) -> SetupRequest {
    SetupRequest {
        ssh: ssh("root"),
        nextcloud_user: "demo".into(),
        folder: folder.into(),
        ctid: None,
        install_dir: None,
        modes: None,
    }
}

#[tokio::test]
async fn the_wizard_falls_back_to_the_host_when_pct_is_missing() {
    let shell = Arc::new(root_without_sudo());
    let nextcloud = Nextcloud::new(Arc::new(FakeConnector::new(shell.clone())));
    let setup = nextcloud
        .set_up(&setup_request("Backup"), "some-transport")
        .await
        .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());

    // The LXC detector's step says why, then the host detector's steps follow.
    let d = &setup.detection;
    assert_eq!(d.detector, HostDetector::ID);
    assert_eq!(
        statuses(d),
        ok(&[
            "pct-list",
            "host",
            "datadirectory",
            "data-path",
            "writable",
            "occ",
            "run-as",
            "owner"
        ])
    );
    assert!(d.steps[0].check.message.contains("not a Proxmox node"));

    let parts = setup.profile.expect("every step passed");
    assert_eq!(parts.destination.root, format!("{FILES}/Backup"));
    assert!(matches!(
        &parts.destination.backend,
        Backend::Sftp { host, port: Some(22), user: Some(u), .. }
            if host == "192.0.2.60" && u == "root"
    ));
    assert_eq!(
        parts.settings["runtime"],
        serde_json::json!({ "kind": "host" })
    );
    nextcloud
        .check_profile(&parts.destination, &parts.settings)
        .unwrap();
    let NextcloudSettings::Direct(nc) = serde_json::from_value(parts.settings.clone()).unwrap()
    else {
        panic!("the wizard saves a direct-mode profile");
    };
    assert_eq!(nc.runtime, Runtime::Host);

    // The saved profile works in direct mode: preflight runs plain host commands.
    let root = format!("{FILES}/Backup");
    shell
        .on(format!("test -d {root}"), Output::ok(""))
        .on(format!("test -w {root}"), Output::ok(""))
        .on(format!("df -Pk {FILES}"), Output::ok(DF))
        .on(
            "su -s /bin/bash www-data -c 'php /var/www/nextcloud/occ user:info demo --output=json'",
            Output::ok(
                r#"{"user_id":"demo","storage":{"free":400,"used":600,"total":1000,"quota":1000}}"#,
            ),
        );
    let ctx = JobContext {
        job_id: "job-1".into(),
        profile: ProfileView {
            id: "p-nc".into(),
            name: "Nextcloud".into(),
            destination: Destination {
                transport: "rclone".into(),
                backend: Backend::Local,
                root,
            },
            settings: parts.settings,
        },
        caps: TransportCaps::empty(),
        checkpoint: None,
    };
    let report = nextcloud.preflight(&ctx).await.unwrap();
    let failed: Vec<_> = report
        .checks
        .iter()
        .filter(|c| c.status != CheckStatus::Ok)
        .collect();
    assert!(failed.is_empty(), "{failed:?}");
    // The user's quota is tighter than the disk, so it is what the report says.
    assert_eq!(
        (report.free_bytes, report.total_bytes, report.used_bytes),
        (Some(400), Some(1000), Some(600))
    );
    assert!(report.checks.iter().any(|c| c.id == "quota"));
    assert_eq!(
        shell.unexpected(),
        Vec::<String>::new(),
        "preflight ran only commands with answers"
    );
}

#[tokio::test]
async fn a_proxmox_node_still_uses_the_lxc_detector() {
    // `pct` exists: the host detector never runs, even when the LXC detection fails.
    let shell = Arc::new(FakeShell::new());
    shell.on(
        "pct list",
        Output::ok("VMID Status Lock Name\n105 stopped  nextcloud\n"),
    );
    let nextcloud = Nextcloud::new(Arc::new(FakeConnector::new(shell.clone())));
    let setup = nextcloud.set_up(&setup_request(""), "t").await.unwrap();
    assert!(setup.profile.is_none());
    assert_eq!(setup.detection.detector, LxcDetector::ID);
    assert_eq!(shell.ran(), ["pct list"]);
}
