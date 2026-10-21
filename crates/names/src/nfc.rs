use unicode_normalization::{UnicodeNormalization, is_nfc};

use crate::{Entry, Issue, NameRule, Severity, TargetProfile};

/// Converts decomposed names (NFD, typical of macOS/iOS and some backups) to NFC.
///
/// Only acts on names that are valid UTF-8; invalid bytes are the `encoding` rule's job.
#[derive(Debug, Default, Clone, Copy)]
pub struct Nfc;

impl Nfc {
    pub const ID: &'static str = "nfc";
}

impl NameRule for Nfc {
    fn id(&self) -> &str {
        Self::ID
    }

    fn applies_to(&self, _target: &TargetProfile) -> bool {
        // NFC is what every target we support expects; Nextcloud enforces it.
        true
    }

    fn check(&self, entry: &Entry) -> Option<Issue> {
        let after = self.propose(entry)?;
        Some(Issue {
            rule: Self::ID,
            severity: Severity::Auto,
            message_key: "names.nfc.decomposed",
            before: entry.raw_name.clone(),
            after: after.into_bytes(),
        })
    }

    fn propose(&self, entry: &Entry) -> Option<String> {
        let name = entry.name.as_deref()?;
        if is_nfc(name) {
            return None;
        }
        Some(name.nfc().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(raw: &[u8]) -> Entry {
        Entry::new(raw.to_vec(), raw.to_vec(), 0, false, None)
    }

    #[test]
    fn nfc_name_is_left_alone() {
        // "Acção" already composed: ç = c3 a7, ã = c3 a3
        let e = file(b"Ac\xc3\xa7\xc3\xa3o.pdf");
        assert_eq!(Nfc.check(&e), None);
        assert_eq!(Nfc.propose(&e), None);
    }

    #[test]
    fn nfd_cedilla_and_tilde_are_composed() {
        // "Acção" decomposed: c + U+0327 (cc a7), a + U+0303 (cc 83)
        let e = file(b"Acc\xcc\xa7a\xcc\x83o.pdf");
        let issue = Nfc.check(&e).expect("NFD name must be flagged");
        assert_eq!(issue.rule, "nfc");
        assert_eq!(issue.severity, Severity::Auto);
        assert_eq!(issue.before, b"Acc\xcc\xa7a\xcc\x83o.pdf");
        assert_eq!(issue.after, b"Ac\xc3\xa7\xc3\xa3o.pdf");
    }

    #[test]
    fn nfd_acute_accent_is_composed() {
        // "Patológica": o + U+0301 (cc 81) -> ó (c3 b3)
        let e = file(b"Pato\xcc\x81gica");
        assert_eq!(Nfc.propose(&e).unwrap().as_bytes(), b"Pat\xc3\xb3gica");
    }

    #[test]
    fn nfd_japanese_dakuten_is_composed() {
        // "が" decomposed: か (e3 81 8b) + U+3099 (e3 82 99) -> が (e3 81 8c)
        let e = file(b"\xe3\x81\x8b\xe3\x82\x99.txt");
        assert_eq!(Nfc.propose(&e).unwrap().as_bytes(), b"\xe3\x81\x8c.txt");
    }

    #[test]
    fn invalid_utf8_is_not_this_rules_job() {
        // Latin-1 "ção" (e7 e3 6f) is not UTF-8.
        let e = file(b"\xe7\xe3o");
        assert_eq!(e.name, None);
        assert_eq!(Nfc.check(&e), None);
    }

    #[test]
    fn plain_ascii_is_left_alone() {
        assert_eq!(Nfc.check(&file(b"report-2024.txt")), None);
    }
}
