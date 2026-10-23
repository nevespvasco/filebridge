//! `DockerDetector` (the official image, Nextcloud AIO) and `SnapDetector` over `FakeShell`,
//! with `docker ps`, `docker inspect` and `snap list` output as the real tools print it, and the
//! wizard trying them between the Proxmox node and Nextcloud on the host.

use std::sync::Arc;

use filebridge_integration_api::CheckStatus;
use filebridge_integration_nextcloud::fake::{FakeConnector, FakeShell};
use filebridge_integration_nextcloud::{
    DetectInput, Detection, Detector, DockerDetector, Nextcloud, NextcloudSettings, Output, Owner,
    Runtime, SetupRequest, SnapDetector, SshTarget,
};

const OCC_STATUS: &str = include_str!("fixtures/occ_status.json");
const PS_AIO: &str = include_str!("fixtures/docker_ps_aio.txt");
const PS_OFFICIAL: &str = include_str!("fixtures/docker_ps_official.txt");
const MOUNTS_AIO: &str = include_str!("fixtures/docker_mounts_aio.json");
const MOUNTS_OFFICIAL: &str = include_str!("fixtures/docker_mounts_official.json");
const CONFIG_AIO: &str = include_str!("fixtures/config_aio.php");
const CONFIG_DOCKER: &str = include_str!("fixtures/config_docker.php");
const CONFIG_SNAP: &str = include_str!("fixtures/config_snap.php");
const SNAP_LIST: &str = include_str!("fixtures/snap_list.txt");

const PS: &str = "docker ps --format '{{.Names}} {{.Image}}'";
const LS_DOCKER: &str = "ls -d /var/www/html/config/config.php \
    /app/www/public/config/config.php /config/www/nextcloud/config/config.php";

fn input(user: &str) -> DetectInput {
    DetectInput {
        ssh: SshTarget {
            host: "192.0.2.70".into(),
            port: 22,
            user: user.into(),
            key_file: None,
            known_hosts_file: None,
            key: None,
        },
        nextcloud_user: "alice".into(),
        ctid: None,
        install_dir: None,
        dedicated_user: false,
    }
}

fn grep(config_php: &str) -> Output {
    Output::ok(
        config_php
            .lines()
            .filter(|l| l.contains("datadirectory"))
            .map(|l| format!("{l}\n"))
            .collect::<String>(),
    )
}

fn ids(d: &Detection) -> Vec<(&str, CheckStatus)> {
    d.steps
        .iter()
        .map(|s| (s.check.id.as_str(), s.check.status))
        .collect()
}

/// Nextcloud AIO on a VM: root over SSH, the data in AIO's `nextcloud_aio_nextcloud_data`
/// volume, `occ` as www-data (33) inside `nextcloud-aio-nextcloud`.
fn aio() -> FakeShell {
    let shell = FakeShell::new();
    let dx = "docker exec nextcloud-aio-nextcloud";
    let vol = "/var/lib/docker/volumes/nextcloud_aio_nextcloud_data/_data";
    shell
        .on(PS, Output::ok(PS_AIO))
        .on(
            "docker inspect --format '{{json .Mounts}}' nextcloud-aio-nextcloud",
            Output::ok(MOUNTS_AIO),
        )
        .on(
            format!("{dx} {LS_DOCKER}"),
            Output {
                status: 2,
                stdout: "/var/www/html/config/config.php\n".into(),
                stderr: "ls: /app/www/public/config/config.php: No such file or directory\n"
                    .into(),
            },
        )
        .on(
            format!("{dx} grep -F datadirectory /var/www/html/config/config.php"),
            grep(CONFIG_AIO),
        )
        .on(format!("{dx} test -d /mnt/ncdata"), Output::ok(""))
        .on(format!("test -d {vol}"), Output::ok(""))
        .on(format!("test -d {vol}/alice/files"), Output::ok(""))
        .on(format!("test -w {vol}/alice/files"), Output::ok(""))
        .on(format!("{dx} test -f /var/www/html/occ"), Output::ok(""))
        .on(
            format!("{dx} stat -c %U /var/www/html/config/config.php"),
            Output::ok("www-data\n"),
        )
        .on(
            "docker exec -u www-data nextcloud-aio-nextcloud php /var/www/html/occ status --output=json",
            Output::ok(OCC_STATUS),
        )
        .on(
            format!("{dx} id www-data"),
            Output::ok("uid=33(www-data) gid=33(www-data) groups=33(www-data)\n"),
        )
        .on(format!("stat -c %u:%g {vol}"), Output::ok("33:0\n"));
    shell
}

#[tokio::test]
async fn nextcloud_aio() {
    let shell = aio();
    let d = DockerDetector.detect(&shell, &input("root")).await;
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert!(d.applies);
    let ok = CheckStatus::Ok;
    assert_eq!(
        ids(&d),
        [
            ("docker", ok),
            ("mounts", ok),
            ("datadirectory", ok),
            ("data-path", ok),
            ("writable", ok),
            ("occ", ok),
            ("run-as", ok),
            // AIO's data folder is 33:0: a warning, the files still get 33:33.
            ("owner", CheckStatus::Warn),
        ]
    );
    assert!(d.steps[0].check.message.contains("Nextcloud AIO"));
    let p = d.profile.unwrap();
    assert_eq!(
        p.runtime,
        Runtime::Docker {
            container: "nextcloud-aio-nextcloud".into()
        }
    );
    assert_eq!(p.data_dir, "/mnt/ncdata");
    assert_eq!(
        p.host_files_dir(),
        "/var/lib/docker/volumes/nextcloud_aio_nextcloud_data/_data/alice/files"
    );
    assert_eq!(p.container_files_dir(), "/mnt/ncdata/alice/files");
    assert_eq!(p.owner, Owner { uid: 33, gid: 33 });
    assert!(!p.host_sudo);
    assert!(p.validate().is_ok());
    assert_eq!(
        p.occ_cmd(&["files:scan", "--path=alice/files/x"]),
        "docker exec -u www-data nextcloud-aio-nextcloud php /var/www/html/occ files:scan --path=alice/files/x"
    );
}

/// The official image from a compose file: a bind mount for the code and one for the data,
/// as a user in the `sudo` group (Docker needs root or its group).
#[tokio::test]
async fn the_official_image_through_sudo() {
    let shell = FakeShell::new();
    let dx = "sudo -n docker exec nextcloud-app-1";
    shell
        .on(
            PS,
            Output::fail(
                1,
                "permission denied while trying to connect to the Docker daemon socket at unix:///var/run/docker.sock\n",
            ),
        )
        .on(format!("sudo -n {PS}"), Output::ok(PS_OFFICIAL))
        .on(
            "sudo -n docker inspect --format '{{json .Mounts}}' nextcloud-app-1",
            Output::ok(MOUNTS_OFFICIAL),
        )
        .on(
            format!("{dx} {LS_DOCKER}"),
            Output::ok("/var/www/html/config/config.php\n"),
        )
        .on(
            format!("{dx} grep -F datadirectory /var/www/html/config/config.php"),
            grep(CONFIG_DOCKER),
        )
        .on(format!("{dx} test -d /var/www/html/data"), Output::ok(""))
        .on("sudo -n test -d /tank/ncdata", Output::ok(""))
        .on("sudo -n test -d /tank/ncdata/alice/files", Output::ok(""))
        .on("test -w /tank/ncdata/alice/files", Output::ok(""))
        .on(format!("{dx} test -f /var/www/html/occ"), Output::ok(""))
        .on(
            format!("{dx} stat -c %U /var/www/html/config/config.php"),
            Output::ok("www-data\n"),
        )
        .on(
            "sudo -n docker exec -u www-data nextcloud-app-1 php /var/www/html/occ status --output=json",
            Output::ok(OCC_STATUS),
        )
        .on(
            format!("{dx} id www-data"),
            Output::ok("uid=33(www-data) gid=33(www-data) groups=33(www-data)\n"),
        )
        .on("sudo -n stat -c %u:%g /tank/ncdata", Output::ok("33:33\n"));
    let d = DockerDetector.detect(&shell, &input("deploy")).await;
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    let p = d.profile.clone().expect("detected");
    assert!(p.host_sudo);
    assert_eq!(p.data_dir, "/var/www/html/data");
    assert_eq!(
        p.host_data_dir, "/tank/ncdata",
        "the data's own mount, not the code's"
    );
    assert!(
        d.steps.iter().all(|s| s.check.status == CheckStatus::Ok),
        "{:?}",
        ids(&d)
    );
}

#[tokio::test]
async fn data_inside_the_container_is_refused() {
    let shell = aio();
    shell.on(
        "docker inspect --format '{{json .Mounts}}' nextcloud-aio-nextcloud",
        Output::ok("[]"),
    );
    let d = DockerDetector.detect(&shell, &input("root")).await;
    let last = d.steps.last().unwrap();
    assert_eq!(
        (last.check.id.as_str(), last.check.status),
        ("datadirectory", CheckStatus::Fail)
    );
    assert!(last.check.message.contains("no volume or bind mount"));
}

#[tokio::test]
async fn docker_without_nextcloud_or_access_lets_the_next_detector_look() {
    // Not installed: nothing said.
    let shell = FakeShell::new();
    shell.on(PS, Output::fail(127, "docker: command not found\n"));
    let d = DockerDetector.detect(&shell, &input("root")).await;
    assert!(!d.applies && d.steps.is_empty());

    // Running other things.
    let shell = FakeShell::new();
    shell.on(
        PS,
        Output::ok("db-1 mariadb:11\nnextcloud-aio-mastercontainer nextcloud/all-in-one:latest\n"),
    );
    let d = DockerDetector.detect(&shell, &input("root")).await;
    assert!(!d.applies);
    assert_eq!(ids(&d), [("docker", CheckStatus::Ok)]);

    // Installed, but this user can't use it: a warning, not a failure.
    let shell = FakeShell::new();
    shell.on(PS, Output::fail(1, "permission denied\n")).on(
        format!("sudo -n {PS}"),
        Output::fail(1, "sudo: a password is required\n"),
    );
    let d = DockerDetector.detect(&shell, &input("alice")).await;
    assert!(!d.applies);
    assert_eq!(ids(&d), [("docker", CheckStatus::Warn)]);
}

#[tokio::test]
async fn two_nextcloud_containers_are_not_guessed_between() {
    let shell = FakeShell::new();
    shell.on(
        PS,
        Output::ok("nc-prod nextcloud:31\nnc-test nextcloud:32\n"),
    );
    let d = DockerDetector.detect(&shell, &input("root")).await;
    assert!(d.applies && d.profile.is_none());
    assert!(d.steps[0].check.message.contains("nc-prod, nc-test"));
}

/// The snap on Ubuntu: root over SSH, the data in the snap's common folder (root-owned: the
/// snap runs Nextcloud as root).
fn snap() -> FakeShell {
    let shell = FakeShell::new();
    let data = "/var/snap/nextcloud/common/nextcloud/data";
    shell
        .on("snap list nextcloud", Output::ok(SNAP_LIST))
        .on("id -u", Output::ok("0\n"))
        .on(
            "grep -F datadirectory /var/snap/nextcloud/current/nextcloud/config/config.php",
            grep(CONFIG_SNAP),
        )
        .on(format!("test -d {data}"), Output::ok(""))
        .on(format!("test -d {data}/alice/files"), Output::ok(""))
        .on(format!("test -w {data}/alice/files"), Output::ok(""))
        .on(
            "/snap/bin/nextcloud.occ status --output=json",
            Output::ok(OCC_STATUS),
        )
        .on(format!("stat -c %u:%g {data}"), Output::ok("0:0\n"));
    shell
}

#[tokio::test]
async fn the_snap() {
    let shell = snap();
    let d = SnapDetector.detect(&shell, &input("root")).await;
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    let ok = CheckStatus::Ok;
    assert_eq!(
        ids(&d),
        [
            ("snap", ok),
            ("host", ok),
            ("datadirectory", ok),
            ("data-path", ok),
            ("writable", ok),
            ("run-as", ok),
            ("owner", ok),
        ]
    );
    assert!(d.steps[0].check.message.contains("30.0.2snap1"));
    let p = d.profile.unwrap();
    assert_eq!(p.runtime, Runtime::Snap);
    assert_eq!(p.owner, Owner { uid: 0, gid: 0 });
    assert_eq!(p.data_dir, p.host_data_dir);
    assert!(p.validate().is_ok());
    assert_eq!(
        p.occ_cmd(&["files:scan", "--path=alice/files"]),
        "/snap/bin/nextcloud.occ files:scan --path=alice/files"
    );
}

#[tokio::test]
async fn no_snap_is_quiet_and_a_non_root_user_is_told() {
    let shell = FakeShell::new();
    shell.on(
        "snap list nextcloud",
        Output::fail(1, "error: no matching snaps installed\n"),
    );
    let d = SnapDetector.detect(&shell, &input("root")).await;
    assert!(!d.applies && d.steps.is_empty());

    let shell = snap();
    shell.on("id -u", Output::ok("1000\n")).on(
        "sudo -n true",
        Output::fail(1, "sudo: a password is required\n"),
    );
    let d = SnapDetector.detect(&shell, &input("alice")).await;
    assert!(d.applies && d.lacks_rights);
    assert_eq!(
        ids(&d),
        [("snap", CheckStatus::Ok), ("host", CheckStatus::Fail)]
    );
}

#[tokio::test]
async fn the_wizard_finds_aio_before_looking_on_the_host() {
    let shell = Arc::new(aio());
    shell
        .on("pct list", Output::fail(127, "pct: command not found\n"))
        .on("test -x /usr/sbin/pct", Output::fail(1, ""));
    let nc = Nextcloud::new(Arc::new(FakeConnector::new(shell.clone())));
    let setup = nc
        .set_up(
            &SetupRequest {
                ssh: input("root").ssh,
                nextcloud_user: "alice".into(),
                folder: "Backup".into(),
                ctid: None,
                install_dir: None,
                modes: None,
                dedicated_user: false,
            },
            "rclone",
        )
        .await
        .unwrap();
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert_eq!(setup.detection.detector, "docker");
    assert_eq!(setup.detection.steps[0].check.id, "pct-list");
    let parts = setup.profile.expect("saved");
    assert_eq!(
        parts.destination.root,
        "/var/lib/docker/volumes/nextcloud_aio_nextcloud_data/_data/alice/files/Backup"
    );
    let NextcloudSettings::Direct(p) = serde_json::from_value(parts.settings).unwrap() else {
        panic!("direct");
    };
    assert!(matches!(p.runtime, Runtime::Docker { .. }));
    // The snap and the host were not looked at once Docker had Nextcloud.
    assert!(!shell.ran().iter().any(|c| c.starts_with("snap ")));
}
