//! A dedicated user instead of root (ADR 0024) over `FakeShell`: not root, in the web server's
//! group, `occ` through a sudoers rule limited to it. Each missing right fails its step with the
//! server setup to run by hand; nothing is ever run on the server to change it.

use std::sync::Arc;

use filebridge_integration_api::CheckStatus;
use filebridge_integration_nextcloud::fake::{FakeConnector, FakeShell};
use filebridge_integration_nextcloud::{
    Access, DetectInput, Detection, Detector, HostDetector, LxcDetector, Nextcloud,
    NextcloudSettings, Output, Owner, RunAs, Runtime, SetupRequest, SshTarget,
};

const CONFIG_PHP: &str = include_str!("fixtures/config_host.php");
const OCC_STATUS: &str = include_str!("fixtures/occ_status.json");

const LS_OCC: &str = "ls -d /var/www/nextcloud/occ /var/www/html/nextcloud/occ \
    /var/www/html/occ /usr/share/webapps/nextcloud/occ /srv/nextcloud/occ /opt/nextcloud/occ";
const FILES: &str = "/mnt/ncdata/demo/files";
const OCC_STATUS_CMD: &str =
    "sudo -n -u www-data /usr/bin/php /var/www/nextcloud/occ status --output=json";

fn ssh() -> SshTarget {
    SshTarget {
        host: "192.0.2.60".into(),
        port: 22,
        user: "filebridge".into(),
        key_file: None,
        known_hosts_file: None,
        key: None,
    }
}

fn input() -> DetectInput {
    DetectInput {
        ssh: ssh(),
        nextcloud_user: "demo".into(),
        ctid: None,
        install_dir: None,
        dedicated_user: true,
    }
}

fn grep_datadirectory() -> Output {
    Output::ok(
        CONFIG_PHP
            .lines()
            .filter(|l| l.contains("datadirectory"))
            .map(|l| format!("{l}\n"))
            .collect::<String>(),
    )
}

/// Everything set up as the server setup says: in group 33, the files folder group-writable,
/// the rule in place.
fn set_up_server() -> FakeShell {
    let shell = FakeShell::new();
    shell
        .on("id -u", Output::ok("1001\n"))
        .on("id -G", Output::ok("1001 33\n"))
        .on(LS_OCC, Output::ok("/var/www/nextcloud/occ\n"))
        .on(
            "stat -c %U /var/www/nextcloud/occ",
            Output::ok("www-data\n"),
        )
        .on("sh -c 'command -v php'", Output::ok("/usr/bin/php\n"))
        .on(
            "id www-data",
            Output::ok("uid=33(www-data) gid=33(www-data) groups=33(www-data)\n"),
        )
        .on("id -gn www-data", Output::ok("www-data\n"))
        .on(
            "grep -F datadirectory /var/www/nextcloud/config/config.php",
            grep_datadirectory(),
        )
        .on("test -d /mnt/ncdata", Output::ok(""))
        .on(format!("test -d {FILES}"), Output::ok(""))
        .on(format!("test -w {FILES}"), Output::ok(""))
        .on(OCC_STATUS_CMD, Output::ok(OCC_STATUS))
        .on("stat -c %u:%g /mnt/ncdata", Output::ok("33:33\n"));
    shell
}

fn statuses(d: &Detection) -> Vec<(&str, CheckStatus)> {
    d.steps
        .iter()
        .map(|s| (s.check.id.as_str(), s.check.status))
        .collect()
}

#[tokio::test]
async fn a_set_up_dedicated_user_needs_no_root() {
    let shell = set_up_server();
    let d = HostDetector.detect(&shell, &input()).await;
    assert_eq!(shell.unexpected(), Vec::<String>::new());
    assert!(
        shell
            .ran()
            .iter()
            .all(|c| !c.starts_with("sudo -n ") || c == OCC_STATUS_CMD),
        "sudo only for occ: {:?}",
        shell.ran()
    );
    let ok = CheckStatus::Ok;
    assert_eq!(
        statuses(&d),
        [
            ("host", ok),
            ("occ", ok),
            ("group", ok),
            ("datadirectory", ok),
            ("data-path", ok),
            ("writable", ok),
            ("run-as", ok),
            ("owner", ok),
        ]
    );
    assert!(!d.lacks_rights);
    let p = d.profile.unwrap();
    assert_eq!(p.access, Access::Dedicated);
    assert_eq!(p.run_as, RunAs::SudoRule);
    assert_eq!(p.runtime, Runtime::Host);
    assert!(!p.host_sudo);
    assert_eq!(p.php, "/usr/bin/php");
    assert_eq!(p.owner, Owner { uid: 33, gid: 33 });
    assert_eq!((p.dir_mode.as_str(), p.file_mode.as_str()), ("770", "660"));
    assert!(p.validate().is_ok());
    // The setup to review is there even when nothing is missing.
    assert!(
        d.server_setup
            .iter()
            .any(|l| l.contains("/usr/bin/php /var/www/nextcloud/occ files\\:scan *"))
    );
    assert!(d.server_setup.contains(&format!("chmod -R g+rwX {FILES}")));
}

#[tokio::test]
async fn not_in_the_group_yet_gets_the_setup() {
    let shell = set_up_server();
    shell.on("id -G", Output::ok("1001\n"));
    let d = HostDetector.detect(&shell, &input()).await;
    assert!(d.profile.is_none() && d.lacks_rights);
    let last = d.steps.last().unwrap();
    assert_eq!(
        (last.check.id.as_str(), last.check.status),
        ("group", CheckStatus::Fail)
    );
    assert!(
        last.check.message.contains("not in www-data's group"),
        "{}",
        last.check.message
    );
    assert!(
        d.server_setup
            .contains(&"usermod -a -G www-data filebridge".to_string())
    );
    // config.php is not readable yet: the folder is a placeholder, never a guess.
    assert!(
        d.server_setup
            .iter()
            .any(|l| l.contains("'<data folder>/demo/files'"))
    );
}

#[tokio::test]
async fn a_missing_sudoers_rule_is_explained_not_added() {
    let shell = set_up_server();
    shell.on(
        OCC_STATUS_CMD,
        Output::fail(1, "sudo: a password is required\n"),
    );
    let d = HostDetector.detect(&shell, &input()).await;
    assert!(d.profile.is_none() && d.lacks_rights);
    let last = d.steps.last().unwrap();
    assert_eq!(last.check.id, "run-as");
    assert!(last.check.message.contains("add the sudoers rule"));
    assert!(last.check.message.contains("password is required"));
    // Nothing touched sudoers: every command run was a read.
    for c in shell.ran() {
        assert!(!c.contains("sudoers") && !c.contains("visudo"), "{c}");
    }
}

#[tokio::test]
async fn a_folder_the_group_cannot_write() {
    let shell = set_up_server();
    shell.on(format!("test -w {FILES}"), Output::fail(1, ""));
    let d = HostDetector.detect(&shell, &input()).await;
    assert!(d.lacks_rights);
    let last = d.steps.last().unwrap();
    assert_eq!(last.check.id, "writable");
    assert!(last.check.message.contains("step 2 of the server setup"));
}

#[tokio::test]
async fn root_is_not_a_dedicated_user() {
    let shell = set_up_server();
    shell.on("id -u", Output::ok("0\n"));
    let d = HostDetector.detect(&shell, &input()).await;
    assert_eq!(statuses(&d), [("host", CheckStatus::Fail)]);
    assert!(d.steps[0].check.message.contains("is root"));
}

#[tokio::test]
async fn an_unreadable_config_php_is_not_taken_for_the_default() {
    let shell = set_up_server();
    shell.on(
        "grep -F datadirectory /var/www/nextcloud/config/config.php",
        Output::fail(
            2,
            "grep: /var/www/nextcloud/config/config.php: Permission denied\n",
        ),
    );
    let d = HostDetector.detect(&shell, &input()).await;
    let last = d.steps.last().unwrap();
    assert_eq!(
        (last.check.id.as_str(), last.check.status),
        ("datadirectory", CheckStatus::Fail)
    );
    assert!(d.lacks_rights);
}

#[tokio::test]
async fn the_wizard_skips_the_proxmox_node_and_saves_a_dedicated_profile() {
    let shell = Arc::new(set_up_server());
    let lxc = LxcDetector.detect(shell.as_ref(), &input()).await;
    assert!(!lxc.applies && lxc.steps.is_empty(), "pct needs root");

    let nc = Nextcloud::new(Arc::new(FakeConnector::new(shell.clone())));
    let setup = nc
        .set_up(
            &SetupRequest {
                ssh: ssh(),
                nextcloud_user: "demo".into(),
                folder: "Backup".into(),
                ctid: None,
                install_dir: None,
                modes: None,
                dedicated_user: true,
            },
            "rclone",
        )
        .await
        .unwrap();
    let parts = setup.profile.expect("saved");
    assert_eq!(parts.destination.root, format!("{FILES}/Backup"));
    let NextcloudSettings::Direct(p) = serde_json::from_value(parts.settings).unwrap() else {
        panic!("direct mode");
    };
    assert_eq!(p.access, Access::Dedicated);
    assert!(!shell.ran().iter().any(|c| c.starts_with("pct")));
}
