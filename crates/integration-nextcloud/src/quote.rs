//! POSIX `sh` quoting for remote commands.
//!
//! An SSH command is one string that the remote login shell parses, so every argument that came
//! from user data (paths with spaces, quotes, `$`, accents) is quoted here. `su -c` adds a second
//! shell, so its command string is built with [`join`] and then quoted again as one argument.

use std::borrow::Cow;

/// Quotes `arg` as one `sh` word. Arguments made only of safe characters are left as they are.
pub fn quote(arg: &str) -> Cow<'_, str> {
    let safe = !arg.is_empty()
        && arg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b));
    if safe {
        return Cow::Borrowed(arg);
    }
    // Inside single quotes nothing is special except `'` itself, written as `'\''`.
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('\'');
    for c in arg.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    Cow::Owned(out)
}

/// Joins `args` into one command line, quoting each.
pub fn join<S: AsRef<str>>(args: &[S]) -> String {
    args.iter()
        .map(|a| quote(a.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Word splitting of a quoted `sh` line, for the subset [`quote`] produces.
    pub(crate) fn split(line: &str) -> Vec<String> {
        let mut words = Vec::new();
        let mut cur = String::new();
        let mut in_word = false;
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match c {
                ' ' => {
                    if in_word {
                        words.push(std::mem::take(&mut cur));
                        in_word = false;
                    }
                }
                '\'' => {
                    in_word = true;
                    for q in chars.by_ref() {
                        if q == '\'' {
                            break;
                        }
                        cur.push(q);
                    }
                }
                '\\' => {
                    in_word = true;
                    cur.push(chars.next().expect("dangling backslash"));
                }
                c => {
                    in_word = true;
                    cur.push(c);
                }
            }
        }
        if in_word {
            words.push(cur);
        }
        words
    }

    const NASTY: &[&str] = &[
        "plain",
        "",
        "with space",
        "O'Brien",
        "say \"hi\"",
        "$HOME `id` $(id)",
        "Verão/Acção",
        "a\\b",
        "*.jpg ?",
        "line\nbreak",
        "-n",
        "'",
        "''",
        "semi;colon&amp|pipe>redir",
    ];

    #[test]
    fn safe_words_are_untouched() {
        assert_eq!(quote("/var/www/nextcloud/occ"), "/var/www/nextcloud/occ");
        assert_eq!(quote("--path=alice/files"), "--path=alice/files");
        assert_eq!(quote("100033:100033"), "100033:100033");
    }

    #[test]
    fn quoting_round_trips() {
        for arg in NASTY {
            assert_eq!(split(&quote(arg)), vec![arg.to_string()], "{arg:?}");
        }
        assert_eq!(split(&join(NASTY)), NASTY);
    }

    #[test]
    fn nested_quoting_round_trips() {
        // `su ... -c '<inner>'`: the inner line is one argument of the outer line.
        let inner = join(&[
            "php",
            "occ",
            "files:scan",
            "--path=alice/files/O'Brien \"x\" $y",
        ]);
        let outer = join(&["su", "-s", "/bin/bash", "www-data", "-c", &inner]);
        let words = split(&outer);
        assert_eq!(words.last().unwrap(), &inner);
        assert_eq!(
            split(&inner).last().unwrap(),
            "--path=alice/files/O'Brien \"x\" $y"
        );
    }

    /// Checks against a real `sh`, two levels deep, as `su -c` would run it.
    #[cfg(unix)]
    #[test]
    fn real_shell_agrees() {
        use std::process::Command;
        for arg in NASTY {
            let inner = join(&["printf", "%s", arg]);
            let outer = join(&["sh", "-c", &inner]);
            let out = Command::new("sh").arg("-c").arg(&outer).output().unwrap();
            assert!(out.status.success(), "{arg:?}: {out:?}");
            assert_eq!(String::from_utf8(out.stdout).unwrap(), *arg, "{arg:?}");
        }
    }
}
