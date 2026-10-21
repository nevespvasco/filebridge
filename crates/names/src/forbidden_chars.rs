use crate::{ActiveFor, Entry, Issue, NameRule, Severity, TargetProfile};

/// Replaces characters that Windows and SMB reject in a name with `_`: `< > : " \ | ? *` and the
/// control characters 0x00–0x1F and 0x7F. (`/` never reaches a name: it is the path separator.)
///
/// Only acts on names that are valid UTF-8; invalid bytes are the `encoding` rule's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForbiddenChars {
    active_for: ActiveFor,
}

impl ForbiddenChars {
    pub const ID: &'static str = "forbidden-chars";
    pub const REPLACEMENT: char = '_';

    pub fn new(active_for: ActiveFor) -> Self {
        Self { active_for }
    }

    fn is_forbidden(c: char) -> bool {
        matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*') || c.is_ascii_control()
    }
}

impl Default for ForbiddenChars {
    /// Active on Windows targets only.
    fn default() -> Self {
        Self::new(ActiveFor::windows())
    }
}

impl NameRule for ForbiddenChars {
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
            message_key: "names.forbidden_chars.replaced",
            before: entry.raw_name.clone(),
            after: after.into_bytes(),
        })
    }

    fn propose(&self, entry: &Entry) -> Option<String> {
        let name = entry.name.as_deref()?;
        if !name.chars().any(Self::is_forbidden) {
            return None;
        }
        Some(
            name.chars()
                .map(|c| {
                    if Self::is_forbidden(c) {
                        Self::REPLACEMENT
                    } else {
                        c
                    }
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TargetFamily, TargetProfile};

    fn file(raw: &[u8]) -> Entry {
        Entry::new(raw.to_vec(), raw.to_vec(), 0, false, None)
    }

    fn rule() -> ForbiddenChars {
        ForbiddenChars::default()
    }

    #[test]
    fn every_forbidden_punctuation_is_replaced() {
        let e = file(b"a<b>c:d\"e\\f|g?h*i.txt");
        let issue = rule()
            .check(&e)
            .expect("forbidden characters must be flagged");
        assert_eq!(issue.rule, "forbidden-chars");
        assert_eq!(issue.severity, Severity::Auto);
        assert_eq!(issue.before, b"a<b>c:d\"e\\f|g?h*i.txt");
        assert_eq!(issue.after, b"a_b_c_d_e_f_g_h_i.txt");
    }

    #[test]
    fn control_characters_are_replaced() {
        // NUL, SOH, TAB, LF, US (0x1f) and DEL (0x7f).
        let e = file(b"\x00a\x01b\tc\nd\x1fe\x7f.txt");
        assert_eq!(rule().propose(&e).unwrap().as_bytes(), b"_a_b_c_d_e_.txt");
    }

    #[test]
    fn multibyte_characters_survive_replacement() {
        // "Relatório: 2024?" -> "Relatório_ 2024_" (ó = c3 b3 stays intact).
        let e = file(b"Relat\xc3\xb3rio: 2024?");
        assert_eq!(
            rule().propose(&e).unwrap().as_bytes(),
            b"Relat\xc3\xb3rio_ 2024_"
        );
    }

    #[test]
    fn clean_name_is_left_alone() {
        // Spaces, dots, dashes, brackets, `#`, `%` and non-ASCII are all fine on Windows.
        let e = file(b"Fatura #12 (c\xc3\xb3pia) [v2] 50%.pdf");
        assert_eq!(rule().check(&e), None);
    }

    #[test]
    fn invalid_utf8_is_not_this_rules_job() {
        // Latin-1 "ção:" is not UTF-8.
        let e = file(b"\xe7\xe3o:");
        assert_eq!(rule().check(&e), None);
    }

    #[test]
    fn default_is_windows_only() {
        let windows = TargetProfile {
            family: TargetFamily::Windows,
            ..TargetProfile::default()
        };
        assert!(rule().applies_to(&windows));
        assert!(!rule().applies_to(&TargetProfile::default()));
    }

    #[test]
    fn integration_can_require_it_on_any_family() {
        let r = ForbiddenChars::new(ActiveFor::windows().and_integrations(["nextcloud"]));
        let nextcloud = TargetProfile {
            integration: Some("nextcloud".into()),
            ..TargetProfile::default()
        };
        let other = TargetProfile {
            integration: Some("other".into()),
            ..TargetProfile::default()
        };
        assert!(r.applies_to(&nextcloud));
        assert!(!r.applies_to(&other));
    }
}
