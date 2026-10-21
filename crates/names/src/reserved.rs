use crate::{ActiveFor, Entry, Issue, NameRule, Severity, TargetProfile};

/// Windows device names. Reserved alone and followed by an extension (`NUL.txt`, `nul.tar.gz`).
///
/// Microsoft's "Naming Files, Paths, and Namespaces" also lists `COM0`/`LPT0` and the
/// superscript-digit forms, so they are included.
pub const WINDOWS_DEVICE_NAMES: &[&str] = &[
    "CON",
    "PRN",
    "AUX",
    "NUL", //
    "COM0",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9", //
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}", //
    "LPT0",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9", //
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// Prefixes `_` to names the destination reserves.
///
/// A list entry matches, ignoring ASCII case, a name equal to it or equal to it up to the first
/// dot: `CON` matches `con` and `con.txt`; `.htaccess` matches only `.htaccess`. The Windows set
/// is [`Reserved::windows`]; an integration builds its own with [`Reserved::new`].
///
/// Only acts on names that are valid UTF-8; invalid bytes are the `encoding` rule's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reserved {
    names: Vec<String>,
    active_for: ActiveFor,
}

impl Reserved {
    pub const ID: &'static str = "reserved";
    pub const PREFIX: char = '_';

    /// `names` reserved on the targets in `active_for`. Empty entries are ignored.
    pub fn new<I, S>(names: I, active_for: ActiveFor) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let names = names
            .into_iter()
            .map(Into::into)
            .filter(|n| !n.is_empty())
            .collect();
        Self { names, active_for }
    }

    /// The Windows device names, active on Windows targets.
    pub fn windows() -> Self {
        Self::new(WINDOWS_DEVICE_NAMES.iter().copied(), ActiveFor::windows())
    }

    fn is_reserved(&self, name: &str) -> bool {
        let stem = name.split('.').next().unwrap_or(name);
        self.names
            .iter()
            .any(|r| r.eq_ignore_ascii_case(name) || r.eq_ignore_ascii_case(stem))
    }
}

impl NameRule for Reserved {
    fn id(&self) -> &str {
        Self::ID
    }

    fn applies_to(&self, target: &TargetProfile) -> bool {
        self.active_for.matches(target)
    }

    fn check(&self, entry: &Entry) -> Option<Issue> {
        let after = self.propose(entry)?;
        Some(Issue {
            rule: Self::ID,
            severity: Severity::Auto,
            message_key: "names.reserved.prefixed",
            before: entry.raw_name.clone(),
            after: after.into_bytes(),
        })
    }

    fn propose(&self, entry: &Entry) -> Option<String> {
        let name = entry.name.as_deref()?;
        if !self.is_reserved(name) {
            return None;
        }
        Some(format!("{}{name}", Self::PREFIX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TargetFamily;

    fn file(raw: &[u8]) -> Entry {
        Entry::new(raw.to_vec(), raw.to_vec(), 0, false, None)
    }

    fn propose(raw: &[u8]) -> Option<String> {
        Reserved::windows().propose(&file(raw))
    }

    #[test]
    fn device_name_is_prefixed() {
        let e = file(b"CON");
        let issue = Reserved::windows().check(&e).expect("CON must be flagged");
        assert_eq!(issue.rule, "reserved");
        assert_eq!(issue.severity, Severity::Auto);
        assert_eq!(issue.before, b"CON");
        assert_eq!(issue.after, b"_CON");
    }

    #[test]
    fn match_ignores_case() {
        assert_eq!(propose(b"nul").as_deref(), Some("_nul"));
        assert_eq!(propose(b"Lpt9").as_deref(), Some("_Lpt9"));
    }

    #[test]
    fn device_name_with_extension_is_prefixed() {
        assert_eq!(propose(b"con.txt").as_deref(), Some("_con.txt"));
        assert_eq!(propose(b"aux.tar.gz").as_deref(), Some("_aux.tar.gz"));
        assert_eq!(propose(b"PRN.").as_deref(), Some("_PRN."));
    }

    #[test]
    fn every_numbered_port_is_reserved() {
        for n in 0..=9 {
            assert!(propose(format!("COM{n}").as_bytes()).is_some(), "COM{n}");
            assert!(
                propose(format!("lpt{n}.log").as_bytes()).is_some(),
                "LPT{n}"
            );
        }
        // COM¹ (c2 b9), LPT³ (c2 b3).
        assert_eq!(propose(b"COM\xc2\xb9").as_deref(), Some("_COM\u{b9}"));
        assert_eq!(
            propose(b"lpt\xc2\xb3.txt").as_deref(),
            Some("_lpt\u{b3}.txt")
        );
    }

    #[test]
    fn longer_or_embedded_names_are_fine() {
        for raw in [
            &b"CONSOLE"[..],
            b"CON1",
            b"COM10",
            b"my.con",
            b"aux_notes.txt",
            b"NUL-backup",
            b" CON",
        ] {
            assert_eq!(propose(raw), None, "{}", String::from_utf8_lossy(raw));
        }
    }

    #[test]
    fn invalid_utf8_is_not_this_rules_job() {
        assert_eq!(propose(b"con.\xe7"), None);
    }

    #[test]
    fn windows_set_is_windows_only() {
        let windows = TargetProfile {
            family: TargetFamily::Windows,
            ..TargetProfile::default()
        };
        assert!(Reserved::windows().applies_to(&windows));
        assert!(!Reserved::windows().applies_to(&TargetProfile::default()));
    }

    #[test]
    fn custom_list_for_an_integration() {
        // How the Nextcloud integration can reuse the type with its own list.
        let r = Reserved::new([".htaccess", ""], ActiveFor::integrations(["nextcloud"]));
        assert_eq!(
            r.propose(&file(b".htaccess")).as_deref(),
            Some("_.htaccess")
        );
        assert_eq!(
            r.propose(&file(b".HTAccess")).as_deref(),
            Some("_.HTAccess")
        );
        assert_eq!(r.propose(&file(b".htaccess.bak")), None);
        assert_eq!(r.propose(&file(b"site.htaccess")), None);
        assert_eq!(r.propose(&file(b"CON")), None, "not the Windows list");
        // The ignored empty entry must not match names that start with a dot.
        assert_eq!(r.propose(&file(b".bashrc")), None);

        let nextcloud = TargetProfile {
            integration: Some("nextcloud".into()),
            ..TargetProfile::default()
        };
        let windows = TargetProfile {
            family: TargetFamily::Windows,
            ..TargetProfile::default()
        };
        assert!(r.applies_to(&nextcloud));
        assert!(!r.applies_to(&windows));
    }
}
