use crate::{Entry, Issue, NameRule, Severity, TargetProfile};

/// Decodes names that are not valid UTF-8, the way old Windows and Linux systems wrote them:
/// as Windows-1252 (CP1252), the superset of Latin-1 those systems used.
///
/// A name with one of the five bytes CP1252 leaves undefined (`81 8d 8f 90 9d`) is read as plain
/// Latin-1 instead. That guess is weaker, so it waits for a decision; the CP1252 one is applied
/// (with the bytes shown, `docs/04`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Encoding;

/// CP1252's characters for the bytes `80`–`9f`; `None` where it defines none.
const CP1252_HIGH: [Option<char>; 32] = [
    Some('\u{20ac}'), // 80 €
    None,             // 81
    Some('\u{201a}'), // 82 ‚
    Some('\u{0192}'), // 83 ƒ
    Some('\u{201e}'), // 84 „
    Some('\u{2026}'), // 85 …
    Some('\u{2020}'), // 86 †
    Some('\u{2021}'), // 87 ‡
    Some('\u{02c6}'), // 88 ˆ
    Some('\u{2030}'), // 89 ‰
    Some('\u{0160}'), // 8a Š
    Some('\u{2039}'), // 8b ‹
    Some('\u{0152}'), // 8c Œ
    None,             // 8d
    Some('\u{017d}'), // 8e Ž
    None,             // 8f
    None,             // 90
    Some('\u{2018}'), // 91 ‘
    Some('\u{2019}'), // 92 ’
    Some('\u{201c}'), // 93 “
    Some('\u{201d}'), // 94 ”
    Some('\u{2022}'), // 95 •
    Some('\u{2013}'), // 96 –
    Some('\u{2014}'), // 97 —
    Some('\u{02dc}'), // 98 ˜
    Some('\u{2122}'), // 99 ™
    Some('\u{0161}'), // 9a š
    Some('\u{203a}'), // 9b ›
    Some('\u{0153}'), // 9c œ
    None,             // 9d
    Some('\u{017e}'), // 9e ž
    Some('\u{0178}'), // 9f Ÿ
];

/// One byte as CP1252, `None` for the five it leaves undefined.
pub(crate) fn cp1252_char(b: u8) -> Option<char> {
    match b {
        0x80..=0x9f => CP1252_HIGH[usize::from(b - 0x80)],
        _ => Some(char::from(b)),
    }
}

/// The CP1252 (or Latin-1) byte for `c`, if it has one: the inverse of [`cp1252_char`], with
/// Latin-1's own `80`–`9f` controls too.
pub(crate) fn cp1252_byte(c: char) -> Option<u8> {
    if let Some(i) = CP1252_HIGH.iter().position(|x| *x == Some(c)) {
        return Some(0x80 + i as u8);
    }
    u8::try_from(u32::from(c)).ok()
}

impl Encoding {
    pub const ID: &'static str = "encoding";

    /// The decoded name and whether it is the sure (CP1252) reading, for a name that is not
    /// UTF-8.
    pub fn decode(raw: &[u8]) -> Option<(String, bool)> {
        if std::str::from_utf8(raw).is_ok() {
            return None;
        }
        let cp1252: Option<String> = raw.iter().map(|b| cp1252_char(*b)).collect();
        Some(match cp1252 {
            Some(name) => (name, true),
            None => (raw.iter().map(|b| char::from(*b)).collect(), false),
        })
    }
}

impl NameRule for Encoding {
    fn id(&self) -> &str {
        Self::ID
    }

    fn applies_to(&self, _target: &TargetProfile) -> bool {
        // Every destination we send to takes UTF-8 names only.
        true
    }

    fn check(&self, entry: &Entry) -> Option<Issue> {
        let (name, sure) = Self::decode(&entry.raw_name)?;
        Some(Issue {
            rule: Self::ID,
            severity: if sure {
                Severity::Auto
            } else {
                Severity::NeedsDecision
            },
            message_key: if sure {
                "names.encoding.decoded"
            } else {
                "names.encoding.guessed"
            },
            before: entry.raw_name.clone(),
            after: name.into_bytes(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(raw: &[u8]) -> Entry {
        Entry::new(raw.to_vec(), raw.to_vec(), 0, false, None)
    }

    #[test]
    fn utf8_names_are_left_alone() {
        assert_eq!(Encoding.check(&file("Acção.pdf".as_bytes())), None);
        assert_eq!(Encoding.check(&file(b"plain.txt")), None);
    }

    #[test]
    fn latin1_name_is_decoded_and_applied() {
        // "Acção" written as Latin-1/CP1252: ç = e7, ã = e3.
        let issue = Encoding.check(&file(b"Ac\xe7\xe3o.pdf")).unwrap();
        assert_eq!(issue.severity, Severity::Auto);
        assert_eq!(issue.after, "Acção.pdf".as_bytes());
        assert_eq!(issue.before, b"Ac\xe7\xe3o.pdf");
    }

    #[test]
    fn cp1252_punctuation_is_decoded() {
        // Windows' curly apostrophe (92) and euro sign (80).
        let issue = Encoding.check(&file(b"Jo\x92s \x80 notes")).unwrap();
        assert_eq!(String::from_utf8(issue.after).unwrap(), "Jo’s € notes");
    }

    #[test]
    fn a_byte_cp1252_lacks_waits_for_a_decision() {
        let issue = Encoding.check(&file(b"a\x81b")).unwrap();
        assert_eq!(issue.severity, Severity::NeedsDecision);
        assert_eq!(issue.message_key, "names.encoding.guessed");
        assert_eq!(String::from_utf8(issue.after).unwrap(), "a\u{81}b");
    }

    #[test]
    fn bytes_and_chars_round_trip() {
        for b in 0u8..=255 {
            if let Some(c) = cp1252_char(b) {
                assert_eq!(cp1252_byte(c), Some(b), "byte {b:#x}");
            }
        }
        assert_eq!(cp1252_byte('\u{81}'), Some(0x81), "Latin-1 control");
        assert_eq!(cp1252_byte('ő'), None, "not in CP1252");
    }
}
