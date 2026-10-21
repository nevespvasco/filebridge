//! Name rules Nextcloud imposes. Active only for targets whose `integration` is `nextcloud`.

use filebridge_names::{Entry, Issue, NameRule, Nfc, Severity, TargetProfile};

use crate::INTEGRATION_ID;

fn for_nextcloud(target: &TargetProfile) -> bool {
    target.integration.as_deref() == Some(INTEGRATION_ID)
}

/// Names must be NFC: Nextcloud refuses NFD entries ("incompatible encoding"). Same fix as the
/// general [`Nfc`] rule, so a job keeps it even if the general rule is turned off.
#[derive(Debug, Default, Clone, Copy)]
pub struct NextcloudNfc;

impl NextcloudNfc {
    pub const ID: &'static str = "nextcloud-nfc";
}

impl NameRule for NextcloudNfc {
    fn id(&self) -> &str {
        Self::ID
    }

    fn applies_to(&self, target: &TargetProfile) -> bool {
        for_nextcloud(target)
    }

    fn check(&self, entry: &Entry) -> Option<Issue> {
        let issue = Nfc.check(entry)?;
        Some(Issue {
            rule: Self::ID,
            message_key: "names.nextcloud_nfc.decomposed",
            ..issue
        })
    }

    fn propose(&self, entry: &Entry) -> Option<String> {
        Nfc.propose(entry)
    }
}

/// Names the Nextcloud server refuses: `.htaccess` (any case), and the `.part` / `.filepart`
/// suffixes it reserves for uploads in progress. `.htaccess` gets a `_` prefix (like the general
/// `reserved` rule); a reserved suffix gets its dot replaced: `video.part` → `video_part`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NextcloudReserved;

impl NextcloudReserved {
    pub const ID: &'static str = "nextcloud-reserved";
    pub const NAMES: &'static [&'static str] = &[".htaccess"];
    pub const SUFFIXES: &'static [&'static str] = &[".part", ".filepart"];
}

impl NameRule for NextcloudReserved {
    fn id(&self) -> &str {
        Self::ID
    }

    fn applies_to(&self, target: &TargetProfile) -> bool {
        for_nextcloud(target)
    }

    fn check(&self, entry: &Entry) -> Option<Issue> {
        let after = self.propose(entry)?;
        Some(Issue {
            rule: Self::ID,
            severity: Severity::Auto,
            message_key: "names.nextcloud_reserved.renamed",
            before: entry.raw_name.clone(),
            after: after.into_bytes(),
        })
    }

    fn propose(&self, entry: &Entry) -> Option<String> {
        let name = entry.name.as_deref()?;
        let lower = name.to_ascii_lowercase();
        if Self::NAMES.contains(&lower.as_str()) {
            return Some(format!("_{name}"));
        }
        let suffix = Self::SUFFIXES.iter().find(|s| lower.ends_with(*s))?;
        let dot = name.len() - suffix.len();
        Some(format!("{}_{}", &name[..dot], &name[dot + 1..]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(raw: &[u8]) -> Entry {
        Entry::new(raw.to_vec(), raw.to_vec(), 0, false, None)
    }

    fn nextcloud() -> TargetProfile {
        TargetProfile {
            integration: Some("nextcloud".into()),
            ..TargetProfile::default()
        }
    }

    #[test]
    fn active_only_for_nextcloud() {
        assert!(NextcloudNfc.applies_to(&nextcloud()));
        assert!(NextcloudReserved.applies_to(&nextcloud()));
        assert!(!NextcloudNfc.applies_to(&TargetProfile::default()));
        assert!(!NextcloudReserved.applies_to(&TargetProfile::default()));
    }

    #[test]
    fn nfd_is_composed_under_own_id() {
        // "Verão" with a + U+0303 (cc 83).
        let issue = NextcloudNfc.check(&file(b"Vera\xcc\x83o")).unwrap();
        assert_eq!(issue.rule, "nextcloud-nfc");
        assert_eq!(issue.after, b"Ver\xc3\xa3o");
        assert_eq!(NextcloudNfc.check(&file(b"Ver\xc3\xa3o")), None);
    }

    #[test]
    fn htaccess_any_case_is_prefixed() {
        assert_eq!(
            NextcloudReserved.propose(&file(b".htaccess")).as_deref(),
            Some("_.htaccess")
        );
        assert_eq!(
            NextcloudReserved.propose(&file(b".HTAccess")).as_deref(),
            Some("_.HTAccess")
        );
        assert_eq!(NextcloudReserved.propose(&file(b"htaccess")), None);
        assert_eq!(NextcloudReserved.propose(&file(b".htaccess.bak")), None);
    }

    #[test]
    fn part_suffixes_lose_their_dot() {
        let issue = NextcloudReserved.check(&file(b"filme.part")).unwrap();
        assert_eq!(issue.rule, "nextcloud-reserved");
        assert_eq!(issue.severity, Severity::Auto);
        assert_eq!(issue.after, b"filme_part");
        assert_eq!(
            NextcloudReserved
                .propose(&file("Ação.FilePart".as_bytes()))
                .as_deref(),
            Some("Ação_FilePart")
        );
        assert_eq!(
            NextcloudReserved.propose(&file(b".part")).as_deref(),
            Some("_part")
        );
        assert_eq!(NextcloudReserved.propose(&file(b"partes.txt")), None);
        assert_eq!(
            NextcloudReserved.propose(&file(b"\xe7.part")),
            None,
            "not UTF-8"
        );
    }
}
