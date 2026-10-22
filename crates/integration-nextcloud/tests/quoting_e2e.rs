//! Runs generated command lines through a real `sh`, with stub `pct`, `su`, `sudo` and `php`
//! that behave like the real ones for argument passing. `php` prints the argv it received, so
//! the test sees exactly what `occ` would get. Nothing leaves the machine.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use filebridge_integration_nextcloud::{NextcloudDirectProfile, Owner, RunAs, Runtime, SshTarget};

const STUBS: &[(&str, &str)] = &[
    // pct exec <id> -- argv…  (runs argv directly, no shell)
    (
        "pct",
        "[ \"$1\" = exec ] && [ \"$3\" = -- ] || exit 90\nshift 3\nexec \"$@\"\n",
    ),
    // su -s <shell> <user> -c <cmd>  (hands <cmd> to a shell)
    (
        "su",
        "[ \"$1\" = -s ] && [ \"$4\" = -c ] || exit 91\nexec /bin/sh -c \"$5\"\n",
    ),
    // sudo [-n] [-u user] argv…
    (
        "sudo",
        "while [ $# -gt 0 ]; do case \"$1\" in -n) shift;; -u) shift 2;; *) break;; esac; done\nexec \"$@\"\n",
    ),
    ("php", "for a in \"$@\"; do printf '%s\\0' \"$a\"; done\n"),
];

fn stubs(dir: &Path) {
    for (name, body) in STUBS {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn profile(runtime: Runtime, run_as: RunAs, host_sudo: bool) -> NextcloudDirectProfile {
    NextcloudDirectProfile {
        ssh: SshTarget {
            host: "pve.lan".into(),
            port: 22,
            user: "root".into(),
            key_file: None,
            known_hosts_file: None,
        },
        host_sudo,
        runtime,
        install_dir: "/var/www/next cloud".into(),
        php: "php".into(),
        run_as,
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

#[test]
fn occ_receives_exact_arguments() {
    let dir = tempfile::tempdir().unwrap();
    stubs(dir.path());
    let path_env = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap()
    );
    let nasty = [
        "--path=alice/files/O'Brien \"festa\"",
        "--path=alice/files/$HOME `id` $(id) ; rm -rf x",
        "--path=alice/files/Verão/Acção e Patológica",
        "--path=alice/files/back\\slash *.jpg ?",
        "--path=alice/files/line\nbreak",
        "--path=alice/files/''",
    ];
    let cases = [
        (RunAs::Su, false),
        (RunAs::Sudo, false),
        (RunAs::Su, true),
        (RunAs::Sudo, true),
    ];
    // Through `pct exec` on a Proxmox node, and on the machine Nextcloud runs on.
    let runtimes = [Runtime::ProxmoxLxc { ctid: 105 }, Runtime::Host];
    for (runtime, (run_as, host_sudo)) in runtimes
        .iter()
        .flat_map(|r| cases.iter().map(move |c| (r.clone(), *c)))
    {
        let p = profile(runtime.clone(), run_as, host_sudo);
        assert_eq!(
            p.occ_cmd(&["status"]).starts_with("pct ")
                || p.occ_cmd(&["status"]).starts_with("sudo -n pct "),
            runtime != Runtime::Host,
            "{runtime:?}"
        );
        for arg in nasty {
            let cmd = p.occ_cmd(&["files:scan", "-v", arg]);
            // What sshd does with the command string: hand it to the login shell.
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(&cmd)
                .env("PATH", &path_env)
                .output()
                .unwrap();
            assert!(out.status.success(), "{cmd}: {out:?}");
            let argv: Vec<String> = String::from_utf8(out.stdout)
                .unwrap()
                .split_terminator('\0')
                .map(String::from)
                .collect();
            assert_eq!(
                argv,
                ["/var/www/next cloud/occ", "files:scan", "-v", arg],
                "{runtime:?} {run_as:?} host_sudo={host_sudo}: {cmd}"
            );
        }
    }
}
