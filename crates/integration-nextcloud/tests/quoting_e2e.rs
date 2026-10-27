//! Runs generated command lines through a real `sh`, with stub `pct`, `su`, `sudo` and `php`
//! that behave like the real ones for argument passing (and pass stdin through, as they do).
//! `php` prints the argv it received, so the test sees exactly what `occ` would get. Nothing
//! leaves the machine.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use filebridge_integration_nextcloud::{NextcloudDirectProfile, Owner, RunAs, Runtime, SshTarget};

const STUBS: &[(&str, &str)] = &[
    // pct exec <id> -- argv…  (runs argv directly, no shell)
    (
        "pct",
        "[ \"$1\" = exec ] && [ \"$3\" = -- ] || exit 90\nshift 3\nexec \"$@\"\n",
    ),
    // su -s <shell> <user> -c <cmd>  (hands <cmd> to <shell>, or `sh` when it is missing)
    (
        "su",
        "[ \"$1\" = -s ] && [ \"$4\" = -c ] || exit 91\n[ -x \"$2\" ] || set -- \"$1\" /bin/sh \"$3\" \"$4\" \"$5\"\nexec \"$2\" -c \"$5\"\n",
    ),
    // sudo [-n] [-u user] argv…
    (
        "sudo",
        "while [ $# -gt 0 ]; do case \"$1\" in -n) shift;; -u) shift 2;; *) break;; esac; done\nexec \"$@\"\n",
    ),
    ("php", "for a in \"$@\"; do printf '%s\\0' \"$a\"; done\n"),
    // docker exec [-i] [-u user] <container> argv…  (runs argv directly, no shell)
    (
        "docker",
        "[ \"$1\" = exec ] || exit 92\nshift\nwhile [ $# -gt 0 ]; do case \"$1\" in -i) shift;; -u) shift 2;; *) break;; esac; done\nshift\nexec \"$@\"\n",
    ),
];

fn stubs(dir: &Path) {
    for (name, body) in STUBS {
        stub(dir, name, body);
    }
}

fn stub(dir: &Path, name: &str, body: &str) {
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn path_env(dir: &Path) -> String {
    format!("{}:{}", dir.display(), std::env::var("PATH").unwrap())
}

/// Every runtime × run-as × host-sudo combination.
fn all_profiles() -> Vec<NextcloudDirectProfile> {
    let mut out = Vec::new();
    for runtime in [Runtime::ProxmoxLxc { ctid: 105 }, Runtime::Host] {
        for run_as in [RunAs::Su, RunAs::Sudo] {
            for host_sudo in [false, true] {
                out.push(profile(runtime.clone(), run_as, host_sudo));
            }
        }
    }
    // A dedicated user (ADR 0024): the watcher runs outside `sudo`, which runs only `php occ`.
    // (The stub `sudo` execs its command, so this proves the plumbing, not sudo's relaying.)
    out.push(profile(Runtime::Host, RunAs::SudoRule, false));
    // Docker (the official image, AIO): `docker exec -i -u www-data <container> sh -c …`.
    for host_sudo in [false, true] {
        out.push(profile(
            Runtime::Docker {
                container: "nextcloud-aio-nextcloud".into(),
            },
            RunAs::Sudo,
            host_sudo,
        ));
    }
    out
}

/// Starts `cmd` the way sshd runs a command without a PTY: `sh -c`, stdin/stdout/stderr pipes.
fn spawn_like_sshd(cmd: &str, path_env: &str, mark: &Path) -> Child {
    Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .env("PATH", path_env)
        .env("MARK", mark)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_at_most(child: &mut Child, limit: Duration, what: &str) -> ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("{what}: still running after {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
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
            key: None,
            password: None,
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
        access: Default::default(),
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
        (RunAs::SudoRule, false),
    ];
    // Through `pct exec` on a Proxmox node, on the machine Nextcloud runs on, and in Docker.
    let runtimes = [
        Runtime::ProxmoxLxc { ctid: 105 },
        Runtime::Host,
        Runtime::Docker {
            container: "nextcloud".into(),
        },
    ];
    for (runtime, (run_as, host_sudo)) in runtimes
        .iter()
        .flat_map(|r| cases.iter().map(move |c| (r.clone(), *c)))
    {
        let p = profile(runtime.clone(), run_as, host_sudo);
        assert_eq!(
            p.occ_cmd(&["status"]).starts_with("pct ")
                || p.occ_cmd(&["status"]).starts_with("sudo -n pct "),
            matches!(runtime, Runtime::ProxmoxLxc { .. }),
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

#[test]
fn long_occ_receives_exact_arguments_and_exits_with_occ() {
    let dir = tempfile::tempdir().unwrap();
    stubs(dir.path());
    let path_env = path_env(dir.path());
    let arg = "--path=alice/files/O'Brien \"festa\" $HOME `id`";
    for p in all_profiles() {
        let cmd = p.occ_cmd_stop_on_eof(&["files:scan", "-v", arg]);
        let mut child = spawn_like_sshd(&cmd, &path_env, Path::new("/dev/null"));
        // The channel stays open while the command runs, as with russh: nothing is written to
        // stdin and it is never closed. The output must still end when `occ` does, so nothing
        // left behind (the watcher) may hold stdout.
        let stdin = child.stdin.take();
        let mut stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            stdout.read_to_end(&mut out).unwrap();
            let _ = tx.send(out);
        });
        let out = rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("output never ended: {cmd}"));
        let status = wait_at_most(&mut child, Duration::from_secs(10), &cmd);
        drop(stdin);
        assert!(status.success(), "{cmd}: {status:?}");
        let argv: Vec<String> = String::from_utf8(out)
            .unwrap()
            .split_terminator('\0')
            .map(String::from)
            .collect();
        assert_eq!(
            argv,
            ["/var/www/next cloud/occ", "files:scan", "-v", arg],
            "{p:?}: {cmd}"
        );
    }
}

#[test]
fn long_occ_stops_when_the_channel_closes() {
    let dir = tempfile::tempdir().unwrap();
    stubs(dir.path());
    // A scan that would take a minute, and records the SIGTERM it gets.
    stub(
        dir.path(),
        "php",
        "trap 'kill $s; echo stopped >> \"$MARK\"; exit 143' TERM\n\
         sleep 60 </dev/null >/dev/null 2>&1 & s=$!\n\
         echo scanning\n\
         wait $s\n",
    );
    let path_env = path_env(dir.path());
    for (i, p) in all_profiles().into_iter().enumerate() {
        let mark = dir.path().join(format!("mark-{i}"));
        let cmd = p.occ_cmd_stop_on_eof(&["files:scan", "-v", "--path=alice/files"]);
        let mut child = spawn_like_sshd(&cmd, &path_env, &mark);
        let mut first = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut first)
            .unwrap();
        assert_eq!(first, "scanning\n", "{cmd}");
        // The service is killed: sshd closes the command's stdin.
        drop(child.stdin.take());
        let status = wait_at_most(&mut child, Duration::from_secs(10), &cmd);
        assert_eq!(status.code(), Some(143), "{p:?}: {cmd}");
        assert_eq!(
            std::fs::read_to_string(&mark).unwrap(),
            "stopped\n",
            "{p:?}: occ got exactly one SIGTERM"
        );
    }
}

#[test]
fn long_occ_without_a_channel_runs_to_the_end() {
    // stdin is /dev/null (no SSH channel to watch): EOF means nothing, `occ` is not stopped.
    let dir = tempfile::tempdir().unwrap();
    stubs(dir.path());
    stub(dir.path(), "php", "sleep 0.2\necho done\n");
    for p in all_profiles() {
        let cmd = p.occ_cmd_stop_on_eof(&["files:scan", "--all"]);
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(&cmd)
            .env("PATH", path_env(dir.path()))
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "{cmd}: {out:?}");
        assert_eq!(out.stdout, b"done\n", "{cmd}");
    }
}
