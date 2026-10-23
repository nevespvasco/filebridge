//! A dedicated user instead of root (ADR 0024): what the user sets up on the server, by hand.
//!
//! filebridge only writes these lines for the user to read and run as root on the machine
//! Nextcloud runs on; it never runs them, and never edits `sudoers` itself. The user:
//! - is in the web server's group, so it can read `config.php` and hand what it sends to that
//!   group (`chgrp`, which needs no root);
//! - can write the folder sends go into, made group-writable once;
//! - may run, through `sudo -u <web user>`, exactly the `occ` commands direct mode uses:
//!   `status --output=json`, `user:info <user> --output=json` and `files:scan …`.

use crate::quote::quote;

/// What [`server_setup`] needs. Unknown values are left as placeholders in angle brackets
/// (`<data folder>`), so a wrong guess never lands in a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupFacts<'a> {
    /// The dedicated SSH user.
    pub ssh_user: &'a str,
    pub web_user: &'a str,
    /// The web user's group name (`id -gn <web user>`), or its gid.
    pub web_group: &'a str,
    /// `php` by its full path, as a sudoers rule must name it.
    pub php: &'a str,
    /// `<install dir>/occ`.
    pub occ: &'a str,
    /// The Nextcloud user whose files receive sends.
    pub nextcloud_user: &'a str,
    /// The user's files folder, when `config.php` could be read.
    pub files_dir: Option<&'a str>,
}

/// Escapes a word for a sudoers command line: `\`, `,`, `:`, `=`, `!`, parentheses and spaces
/// are special there.
pub fn sudoers_escape(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for c in word.chars() {
        if matches!(
            c,
            '\\' | ',' | ':' | '=' | '!' | '(' | ')' | ' ' | '\t' | '#' | '"'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A sudoers user or group name filebridge is willing to write into a rule.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// The sudoers line: `occ` as the web user, only the commands direct mode runs. `None` when a
/// name holds characters a rule shouldn't (filebridge then writes no rule at all).
pub fn sudoers_rule(f: &SetupFacts<'_>) -> Option<String> {
    if !plain_name(f.ssh_user) || !plain_name(f.web_user) || f.nextcloud_user.contains('\0') {
        return None;
    }
    if !f.php.starts_with('/') || !f.occ.starts_with('/') {
        return None;
    }
    let cmd = |args: &str| format!("{} {} {args}", sudoers_escape(f.php), sudoers_escape(f.occ));
    Some(format!(
        "{} ALL=({}) NOPASSWD: {}, {}, {}",
        f.ssh_user,
        f.web_user,
        cmd("status --output\\=json"),
        cmd(&format!(
            "user\\:info {} --output\\=json",
            sudoers_escape(f.nextcloud_user)
        )),
        cmd("files\\:scan *"),
    ))
}

/// The commands the user runs as root on the server, with comments, one line each.
pub fn server_setup(f: &SetupFacts<'_>) -> Vec<String> {
    let user = quote(f.ssh_user);
    let group = quote(f.web_group);
    let mut out = vec![
        "# Read these, then run them yourself as root on the machine Nextcloud runs on.".into(),
        "# filebridge never runs them and never changes sudoers itself.".into(),
        format!(
            "# 1. The dedicated user, in the web server's group ({}):",
            f.web_group
        ),
        format!("id {user} >/dev/null 2>&1 || useradd --create-home --shell /bin/sh {user}"),
        format!("usermod -a -G {group} {user}"),
        "# 2. The folder filebridge sends into, writable by that group:".into(),
    ];
    match f.files_dir {
        Some(dir) => out.push(format!("chmod -R g+rwX {}", quote(dir))),
        None => out.push(format!(
            "chmod -R g+rwX '<data folder>/{}/files'   # the folder from config.php's datadirectory",
            f.nextcloud_user
        )),
    }
    match sudoers_rule(f) {
        Some(rule) => {
            out.push(format!(
                "# 3. occ as {}, and nothing else (visudo checks the file):",
                f.web_user
            ));
            out.push(format!(
                "printf '%s\\n' {} > /etc/sudoers.d/filebridge",
                quote(&rule)
            ));
            out.push("chmod 0440 /etc/sudoers.d/filebridge".into());
            out.push("visudo -cf /etc/sudoers.d/filebridge".into());
        }
        None => out.push(
            "# 3. No sudoers rule written: a user name or path has characters a rule can't hold."
                .into(),
        ),
    }
    out.push(
        "# 4. Sign in as that user with your SSH key (or filebridge's own), then Detect again."
            .into(),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> SetupFacts<'static> {
        SetupFacts {
            ssh_user: "filebridge",
            web_user: "www-data",
            web_group: "www-data",
            php: "/usr/bin/php",
            occ: "/var/www/nextcloud/occ",
            nextcloud_user: "alice",
            files_dir: Some("/mnt/ncdata/alice/files"),
        }
    }

    #[test]
    fn the_rule_names_only_occ() {
        assert_eq!(
            sudoers_rule(&facts()).unwrap(),
            "filebridge ALL=(www-data) NOPASSWD: \
             /usr/bin/php /var/www/nextcloud/occ status --output\\=json, \
             /usr/bin/php /var/www/nextcloud/occ user\\:info alice --output\\=json, \
             /usr/bin/php /var/www/nextcloud/occ files\\:scan *"
        );
    }

    #[test]
    fn special_characters_are_escaped_or_refused() {
        let f = SetupFacts {
            occ: "/srv/next cloud/occ",
            nextcloud_user: "a,b",
            ..facts()
        };
        let rule = sudoers_rule(&f).unwrap();
        assert!(rule.contains("/srv/next\\ cloud/occ"), "{rule}");
        assert!(rule.contains("user\\:info a\\,b"), "{rule}");
        for bad in [
            SetupFacts {
                ssh_user: "root ALL=(ALL) ALL #",
                ..facts()
            },
            SetupFacts {
                web_user: "www data",
                ..facts()
            },
            SetupFacts {
                php: "php",
                ..facts()
            },
        ] {
            assert_eq!(sudoers_rule(&bad), None);
            assert!(server_setup(&bad).iter().all(|l| !l.contains("sudoers.d")));
        }
    }

    #[test]
    fn setup_is_for_the_user_to_run() {
        let lines = server_setup(&facts());
        assert!(lines[0].starts_with("# Read these"));
        assert!(lines.contains(&"usermod -a -G www-data filebridge".to_string()));
        assert!(lines.contains(&"chmod -R g+rwX /mnt/ncdata/alice/files".to_string()));
        assert!(lines.contains(&"visudo -cf /etc/sudoers.d/filebridge".to_string()));
        let printf = lines.iter().find(|l| l.starts_with("printf")).unwrap();
        // The rule goes through the shell as one quoted word, backslashes intact.
        assert!(
            printf.contains("'filebridge ALL=(www-data) NOPASSWD: "),
            "{printf}"
        );
        assert!(printf.contains("files\\:scan *'"), "{printf}");

        let unknown = server_setup(&SetupFacts {
            files_dir: None,
            ..facts()
        });
        assert!(
            unknown
                .iter()
                .any(|l| l.contains("'<data folder>/alice/files'"))
        );
    }
}
