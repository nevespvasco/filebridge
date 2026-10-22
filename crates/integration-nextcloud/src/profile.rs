//! The saved direct-mode profile, and how it turns into remote command lines.

use filebridge_integration_api::{IntegrationError, Result};
use serde::{Deserialize, Serialize};

use crate::quote::join;
use crate::shell::SshTarget;

/// What the Nextcloud integration stores in `ProfileView::settings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum NextcloudSettings {
    /// SFTP into the data folder; owner/mode and `occ files:scan` done over SSH.
    Direct(Box<NextcloudDirectProfile>),
    /// Nextcloud's own WebDAV API with an app password (docs/05: the default mode).
    Webdav(crate::webdav::NextcloudWebdavProfile),
}

/// Where Nextcloud runs, seen from the SSH host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Runtime {
    /// The SSH host is a Proxmox node; Nextcloud is in container `ctid` (`pct exec`).
    ProxmoxLxc { ctid: u32 },
    /// Nextcloud runs on the SSH host itself (its LXC or VM reached directly, or a bare-metal
    /// server): "container" commands are plain host commands, and both views of a path match.
    Host,
}

/// How to become the web-server user where Nextcloud runs, to run `occ`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunAs {
    /// `sudo -n -u <web_user> php occ …`
    Sudo,
    /// `su -s /bin/bash <web_user> -c 'php occ …'` (machines without sudo).
    Su,
}

/// Numeric owner on the SSH host (after the container's idmap, for [`Runtime::ProxmoxLxc`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Owner {
    pub uid: u32,
    pub gid: u32,
}

impl Owner {
    pub fn chown_spec(&self) -> String {
        format!("{}:{}", self.uid, self.gid)
    }
}

/// Everything direct mode needs, found by a [`crate::detect::Detector`] and saved in the profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextcloudDirectProfile {
    pub ssh: SshTarget,
    /// Host commands need `sudo -n` (the SSH user is not root).
    pub host_sudo: bool,
    pub runtime: Runtime,
    /// Nextcloud install directory where Nextcloud runs (holds `occ` and `config/`).
    pub install_dir: String,
    pub php: String,
    pub run_as: RunAs,
    pub web_user: String,
    /// `datadirectory` from `config.php`, as the container sees it.
    pub data_dir: String,
    /// The same directory as the SSH host sees it (through the bind mount); equal to
    /// `data_dir` for [`Runtime::Host`].
    pub host_data_dir: String,
    /// Nextcloud user id whose `files/` receives the send.
    pub user: String,
    pub owner: Owner,
    /// Octal modes applied after the transfer, e.g. `"750"` and `"640"`.
    pub dir_mode: String,
    pub file_mode: String,
}

impl NextcloudDirectProfile {
    pub const DEFAULT_DIR_MODE: &'static str = "750";
    pub const DEFAULT_FILE_MODE: &'static str = "640";

    /// Rejects values that would make a command wrong (modes, empty paths, NUL bytes).
    pub fn validate(&self) -> Result<()> {
        let octal =
            |m: &str| (3..=4).contains(&m.len()) && m.bytes().all(|b| (b'0'..=b'7').contains(&b));
        if !octal(&self.dir_mode) || !octal(&self.file_mode) {
            return Err(IntegrationError::fatal(format!(
                "invalid modes {:?}/{:?}: expected octal like 750/640",
                self.dir_mode, self.file_mode
            )));
        }
        for (what, v) in [
            ("install_dir", &self.install_dir),
            ("data_dir", &self.data_dir),
            ("host_data_dir", &self.host_data_dir),
        ] {
            if !v.starts_with('/') {
                return Err(IntegrationError::fatal(format!(
                    "{what} must be absolute: {v:?}"
                )));
            }
        }
        if self.runtime == Runtime::Host && self.host_data_dir != self.data_dir {
            return Err(IntegrationError::fatal(format!(
                "Nextcloud runs on the SSH host, so host_data_dir {:?} must equal data_dir {:?}",
                self.host_data_dir, self.data_dir
            )));
        }
        if self.user.is_empty() || self.user.contains('/') || self.user.starts_with('.') {
            return Err(IntegrationError::fatal(format!(
                "invalid Nextcloud user {:?}",
                self.user
            )));
        }
        let all = [
            &self.install_dir,
            &self.php,
            &self.web_user,
            &self.data_dir,
            &self.host_data_dir,
            &self.user,
        ];
        if all.iter().any(|v| v.contains('\0')) {
            return Err(IntegrationError::fatal(
                "profile values must not contain NUL bytes",
            ));
        }
        Ok(())
    }

    /// `<host_data_dir>/<user>/files`, where Nextcloud keeps the user's files.
    pub fn host_files_dir(&self) -> String {
        path_join(&path_join(&self.host_data_dir, &self.user), "files")
    }

    /// `<data_dir>/<user>/files`, as the container sees it.
    pub fn container_files_dir(&self) -> String {
        path_join(&path_join(&self.data_dir, &self.user), "files")
    }

    pub fn occ_path(&self) -> String {
        path_join(&self.install_dir, "occ")
    }

    fn exec(&self) -> Exec {
        Exec {
            host_sudo: self.host_sudo,
            runtime: self.runtime.clone(),
        }
    }

    /// A command on the SSH host.
    pub fn host_cmd<S: AsRef<str>>(&self, argv: &[S]) -> String {
        self.exec().host(argv)
    }

    /// A command where Nextcloud runs (inside the container, as its root; or on the host).
    pub fn container_cmd<S: AsRef<str>>(&self, argv: &[S]) -> String {
        self.exec().container(argv)
    }

    /// `occ <args>` as the web-server user.
    pub fn occ_cmd<S: AsRef<str>>(&self, args: &[S]) -> String {
        self.exec().occ(
            self.run_as,
            &self.web_user,
            &self.php,
            &self.occ_path(),
            args,
            false,
        )
    }

    /// Like [`Self::occ_cmd`], but `occ` gets SIGTERM when the SSH channel closes (see
    /// [`stop_on_eof`]). For long commands (`files:scan`), so a service restart or a cancelled job
    /// leaves no `occ` running on the server.
    pub fn occ_cmd_stop_on_eof<S: AsRef<str>>(&self, args: &[S]) -> String {
        self.exec().occ(
            self.run_as,
            &self.web_user,
            &self.php,
            &self.occ_path(),
            args,
            true,
        )
    }
}

/// Builds command lines; shared by the profile and the detector (which has no profile yet).
#[derive(Debug, Clone)]
pub(crate) struct Exec {
    pub host_sudo: bool,
    pub runtime: Runtime,
}

impl Exec {
    pub fn host<S: AsRef<str>>(&self, argv: &[S]) -> String {
        let mut words: Vec<&str> = Vec::with_capacity(argv.len() + 2);
        if self.host_sudo {
            words.extend(["sudo", "-n"]);
        }
        words.extend(argv.iter().map(AsRef::as_ref));
        join(&words)
    }

    /// A command where Nextcloud runs: `pct exec <id> -- argv…` for an LXC, the host command
    /// itself for [`Runtime::Host`].
    pub fn container<S: AsRef<str>>(&self, argv: &[S]) -> String {
        match self.runtime {
            Runtime::ProxmoxLxc { ctid } => {
                let ctid = ctid.to_string();
                // `pct exec` runs argv directly (no shell inside the container).
                let mut words: Vec<&str> = vec!["pct", "exec", &ctid, "--"];
                words.extend(argv.iter().map(AsRef::as_ref));
                self.host(&words)
            }
            Runtime::Host => self.host(argv),
        }
    }

    /// `stop_on_eof` wraps `php occ …` in [`stop_on_eof`], run as the web user right next to
    /// `php`: no `sudo`, `su` or `pct exec` in between has to relay the signal, and the web user
    /// may always signal its own process.
    pub fn occ<S: AsRef<str>>(
        &self,
        run_as: RunAs,
        web_user: &str,
        php: &str,
        occ: &str,
        args: &[S],
        stop_on_eof: bool,
    ) -> String {
        let mut occ_argv: Vec<&str> = vec![php, occ];
        occ_argv.extend(args.iter().map(AsRef::as_ref));
        let line = || {
            if stop_on_eof {
                self::stop_on_eof(&occ_argv)
            } else {
                join(&occ_argv)
            }
        };
        match run_as {
            RunAs::Sudo if stop_on_eof => {
                self.container(&["sudo", "-n", "-u", web_user, "sh", "-c", &line()])
            }
            RunAs::Sudo => {
                let mut argv = vec!["sudo", "-n", "-u", web_user];
                argv.extend(&occ_argv);
                self.container(&argv)
            }
            RunAs::Su => {
                // `su -c` hands the string to bash: quote it as a command line, then as one word.
                self.container(&["su", "-s", "/bin/bash", web_user, "-c", &line()])
            }
        }
    }
}

/// An `sh` line that runs `argv` and sends it SIGTERM once the line's stdin reaches EOF.
///
/// Over SSH, stdin is the channel, which filebridge never writes to nor closes: EOF comes only
/// when sshd closes it, because the channel or the whole connection is gone (job cancelled,
/// service killed or restarted). Without this the remote command outlives the connection: sshd
/// signals nothing, and PHP ignores SIGPIPE, so not even writing to the dead channel stops it.
/// `occ` traps SIGTERM when PHP has `pcntl` (`files:scan` then stops at the next file);
/// otherwise PHP just exits.
///
/// The watcher only starts when stdin is a pipe or socket (what sshd gives a command without a
/// PTY); with any other stdin (`/dev/null`, a terminal) EOF would mean nothing and `argv` simply
/// runs to the end. `argv` itself reads `/dev/null`, and the watcher holds no copy of stdout or
/// stderr, so sshd sees the output end as soon as `argv` exits.
pub(crate) fn stop_on_eof<S: AsRef<str>>(argv: &[S]) -> String {
    format!(
        "exec 3<&0; {} </dev/null & p=$!; w=; \
         if [ -p /dev/stdin ] || [ -S /dev/stdin ]; then \
         (while read -r l; do :; done <&3; kill -TERM $p) >/dev/null 2>&1 & w=$!; fi; \
         wait $p; s=$?; [ -z \"$w\" ] || kill $w 2>/dev/null; exit $s",
        join(argv)
    )
}

/// Joins two `/` paths without doubling or dropping the separator. `b` empty returns `a`.
pub(crate) fn path_join(a: &str, b: &str) -> String {
    match (a.trim_end_matches('/'), b.trim_start_matches('/')) {
        (a, "") => {
            if a.is_empty() {
                "/".into()
            } else {
                a.into()
            }
        }
        ("", b) => format!("/{b}"),
        (a, b) => format!("{a}/{b}"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample() -> NextcloudDirectProfile {
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
                uid: 100033,
                gid: 100033,
            },
            dir_mode: "750".into(),
            file_mode: "640".into(),
        }
    }

    #[test]
    fn occ_through_su_is_double_quoted() {
        use crate::quote::tests::split;
        let path = "--path=alice/files/Fotos/O'Neil \"Verão\" $HOME";
        let cmd = sample().occ_cmd(&["files:scan", path]);
        let words = split(&cmd);
        assert_eq!(
            words[..9],
            [
                "pct",
                "exec",
                "105",
                "--",
                "su",
                "-s",
                "/bin/bash",
                "www-data",
                "-c"
            ]
        );
        assert_eq!(words.len(), 10);
        assert_eq!(
            split(&words[9]),
            ["php", "/var/www/nextcloud/occ", "files:scan", path]
        );
    }

    #[test]
    fn occ_through_sudo_and_host_sudo() {
        let mut p = sample();
        p.run_as = RunAs::Sudo;
        p.host_sudo = true;
        assert_eq!(
            p.occ_cmd(&["status", "--output=json"]),
            "sudo -n pct exec 105 -- sudo -n -u www-data php /var/www/nextcloud/occ status --output=json"
        );
        assert_eq!(p.host_cmd(&["df", "-Pk", "/a b"]), "sudo -n df -Pk '/a b'");
    }

    #[test]
    fn stop_on_eof_wraps_php_as_the_web_user() {
        use crate::quote::tests::split;
        let path = "--path=alice/files/O'Neil $HOME";
        let php = ["php", "/var/www/nextcloud/occ", "files:scan", path];
        let line = stop_on_eof(&php);
        assert!(line.contains(&format!("; {} </dev/null & p=$!;", join(&php))));

        let mut p = sample();
        let words = split(&p.occ_cmd_stop_on_eof(&["files:scan", path]));
        assert_eq!(
            words[..9],
            [
                "pct",
                "exec",
                "105",
                "--",
                "su",
                "-s",
                "/bin/bash",
                "www-data",
                "-c"
            ]
        );
        assert_eq!(words[9..], *std::slice::from_ref(&line));

        p.run_as = RunAs::Sudo;
        p.host_sudo = true;
        let words = split(&p.occ_cmd_stop_on_eof(&["files:scan", path]));
        assert_eq!(
            words,
            [
                "sudo", "-n", "pct", "exec", "105", "--", "sudo", "-n", "-u", "www-data", "sh",
                "-c", &line
            ]
        );

        let mut h = sample_host();
        assert_eq!(
            split(&h.occ_cmd_stop_on_eof(&["files:scan", path])),
            ["su", "-s", "/bin/bash", "www-data", "-c", &line]
        );
        h.run_as = RunAs::Sudo;
        assert_eq!(
            split(&h.occ_cmd_stop_on_eof(&["files:scan", path])),
            ["sudo", "-n", "-u", "www-data", "sh", "-c", &line]
        );
        // Short commands stay as they were.
        assert_eq!(
            h.occ_cmd(&["status"]),
            "sudo -n -u www-data php /var/www/nextcloud/occ status"
        );
    }

    pub fn sample_host() -> NextcloudDirectProfile {
        NextcloudDirectProfile {
            ssh: SshTarget {
                host: "nextcloud.tailnet".into(),
                ..sample().ssh
            },
            runtime: Runtime::Host,
            data_dir: "/mnt/ncdata".into(),
            host_data_dir: "/mnt/ncdata".into(),
            owner: Owner { uid: 33, gid: 33 },
            ..sample()
        }
    }

    #[test]
    fn host_runtime_runs_commands_as_they_are() {
        let mut p = sample_host();
        assert_eq!(p.container_cmd(&["test", "-d", "/a b"]), "test -d '/a b'");
        assert_eq!(
            p.occ_cmd(&["status"]),
            "su -s /bin/bash www-data -c 'php /var/www/nextcloud/occ status'"
        );
        p.run_as = RunAs::Sudo;
        p.host_sudo = true;
        assert_eq!(
            p.occ_cmd(&["status"]),
            "sudo -n sudo -n -u www-data php /var/www/nextcloud/occ status"
        );
        assert_eq!(p.container_files_dir(), p.host_files_dir());
    }

    #[test]
    fn settings_json_shape() {
        let s = NextcloudSettings::Direct(Box::new(sample()));
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["mode"], "direct");
        assert_eq!(v["runtime"]["kind"], "proxmox_lxc");
        assert_eq!(v["run_as"], "su");
        let back: NextcloudSettings = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);

        let h = NextcloudSettings::Direct(Box::new(sample_host()));
        let v = serde_json::to_value(&h).unwrap();
        assert_eq!(v["runtime"], serde_json::json!({ "kind": "host" }));
        assert_eq!(serde_json::from_value::<NextcloudSettings>(v).unwrap(), h);
    }

    #[test]
    fn validation() {
        assert!(sample().validate().is_ok());
        let mut p = sample();
        p.dir_mode = "75; rm".into();
        assert!(p.validate().is_err());
        let mut p = sample();
        p.user = "../x".into();
        assert!(p.validate().is_err());
        let mut p = sample();
        p.host_data_dir = "relative".into();
        assert!(p.validate().is_err());
        assert!(sample_host().validate().is_ok());
        let mut p = sample_host();
        p.host_data_dir = "/tank/nextcloud-data".into();
        assert!(p.validate().is_err(), "one machine sees one data path");
    }

    #[test]
    fn paths() {
        let p = sample();
        assert_eq!(p.host_files_dir(), "/tank/nextcloud-data/alice/files");
        assert_eq!(p.container_files_dir(), "/mnt/ncdata/alice/files");
        assert_eq!(path_join("/a/", "/b"), "/a/b");
        assert_eq!(path_join("/a", ""), "/a");
        assert_eq!(path_join("/", ""), "/");
        assert_eq!(path_join("/", "x"), "/x");
    }
}
