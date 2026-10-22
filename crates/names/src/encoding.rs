use crate::{Entry, Issue, NameRule, Severity, TargetProfile};

/// Decodes names that are not valid UTF-8, the way old Windows and Linux systems wrote them:
/// as Windows-1252 (CP1252), the superset of Latin-1 those systems used.
///
/// The CP1252 reading is applied by itself (with the bytes shown, `docs/04`) only when it looks
/// like a name written in a Latin script: every non-ASCII character a Latin-1 letter or common
/// punctuation, and never more than two of them in a row. Bytes from another code page (CP850,
/// CP1251, Shift-JIS, GBK…) read as CP1252 rarely look like that, so any other name waits for a
/// decision with the CP1252 reading as the proposal. The five bytes CP1252 leaves undefined
/// (`81 8d 8f 90 9d`) become `_` in it: Latin-1 would read them as control characters, which no
/// name should carry.
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

/// What an undefined CP1252 byte becomes in the proposal.
const UNDEFINED: char = '_';

/// Non-ASCII characters a Latin-script name commonly holds, beyond the Latin-1 letters: CP1252's
/// own letters and the punctuation people type in names.
const PLAUSIBLE_EXTRA: &str = "ŒœŠšŽžŸ‘’“”–—…€•°«»ªº§©®™¡¿£´·¹²³½¼¾";

/// Whether `c` is a letter or a symbol a Latin-script name would hold. The Latin-1 letters
/// minus `× ÷` and the Icelandic `Ð ð Þ þ`, which CP1251 Cyrillic lands on most often.
fn plausible(c: char) -> bool {
    c.is_ascii()
        || (('\u{c0}'..='\u{ff}').contains(&c) && !"×÷ÐðÞþ".contains(c))
        || PLAUSIBLE_EXTRA.contains(c)
}

/// Whether a CP1252 reading looks like a name in a Latin script: every character plausible, and
/// at most two non-ASCII ones in a row (`ção` has two; Cyrillic or CJK read as CP1252 has runs).
fn looks_latin(name: &str) -> bool {
    let mut run = 0;
    for c in name.chars() {
        if !plausible(c) {
            return false;
        }
        run = if c.is_ascii() { 0 } else { run + 1 };
        if run > 2 {
            return false;
        }
    }
    true
}

impl Encoding {
    pub const ID: &'static str = "encoding";

    /// The CP1252 reading of a name that is not UTF-8, and whether it is sure enough to apply by
    /// itself (it looks like a Latin-script name and every byte is defined).
    pub fn decode(raw: &[u8]) -> Option<(String, bool)> {
        if std::str::from_utf8(raw).is_ok() {
            return None;
        }
        let mut defined = true;
        let name: String = raw
            .iter()
            .map(|b| {
                cp1252_char(*b).unwrap_or_else(|| {
                    defined = false;
                    UNDEFINED
                })
            })
            .collect();
        let sure = defined && looks_latin(&name);
        Some((name, sure))
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
    fn a_byte_cp1252_lacks_waits_for_a_decision_and_never_becomes_a_control() {
        let issue = Encoding.check(&file(b"a\x81b")).unwrap();
        assert_eq!(issue.severity, Severity::NeedsDecision);
        assert_eq!(issue.message_key, "names.encoding.guessed");
        assert_eq!(String::from_utf8(issue.after).unwrap(), "a_b");
    }

    #[test]
    fn other_code_pages_wait_for_a_decision() {
        for (raw, what) in [
            (&b"\xcf\xf0\xe8\xe2\xe5\xf2.txt"[..], "CP1251 Привет"),
            (&b"\x93\xfa\x96\x7b.txt"[..], "Shift-JIS 日本"),
            (&b"\xd6\xd0\xce\xc4.txt"[..], "GBK 中文"),
            (&b"Ac\x87\xc6o.txt"[..], "CP850 Acção"),
            (&b"caf\x82.txt"[..], "CP850 café"),
        ] {
            let issue = Encoding.check(&file(raw)).unwrap();
            assert_eq!(issue.severity, Severity::NeedsDecision, "{what}");
            assert!(
                std::str::from_utf8(&issue.after).is_ok(),
                "{what}: still a proposal"
            );
        }
    }

    #[test]
    fn latin_names_with_accents_and_punctuation_are_sure() {
        for raw in [
            &b"Gr\xf6\xdfe.txt"[..],         // Größe
            &b"\xc9cole \xab test \xbb"[..], // École « test »
            &b"Jo\x92s"[..],                 // Jo’s
            &b"na\xefve caf\xe9"[..],        // naïve café
        ] {
            let issue = Encoding.check(&file(raw)).unwrap();
            assert_eq!(issue.severity, Severity::Auto, "{raw:?}");
        }
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
