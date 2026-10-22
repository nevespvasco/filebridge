use crate::encoding::cp1252_byte;
use crate::{Entry, Issue, NameRule, Severity, TargetProfile};

/// Finds names whose UTF-8 bytes were once read as CP1252 or Latin-1 and written back
/// (`PatolÃ³gica` for `Patológica`), and proposes the original (`docs/04`, the `ftfy` approach).
///
/// The name is re-encoded as CP1252/Latin-1 and the bytes read as UTF-8; the result is kept only
/// if it is valid **and** looks less garbled than the input. At most two rounds, for names
/// garbled twice. A proposal, never applied by itself: a real name can look like this.
#[derive(Debug, Default, Clone, Copy)]
pub struct Mojibake;

/// Rounds of reversal tried, for names garbled more than once.
const MAX_ROUNDS: usize = 2;

/// How garbled a name looks: C1 control characters, plus each character that is a UTF-8 lead
/// byte read as Latin-1 (`Ã`, `Â`, `â`…) followed by one that is a continuation byte read the
/// same way (`³`, `©`, `€`…). A real name almost never has either. A reversal can also land on
/// odd characters (unassigned code points, stray marks) or on a letter of another script amid
/// Latin ones (`Ø¨` → `ب`); those count too, so such a reversal never looks better.
fn weirdness(s: &str) -> usize {
    let lead = |c: char| ('\u{c2}'..='\u{f4}').contains(&c);
    let continuation = |c: char| cp1252_byte(c).is_some_and(|b| (0x80..=0xbf).contains(&b));
    let controls = s
        .chars()
        .filter(|c| ('\u{80}'..='\u{9f}').contains(c))
        .count();
    let chars: Vec<char> = s.chars().collect();
    let pairs = chars
        .windows(2)
        .filter(|w| lead(w[0]) && continuation(w[1]))
        .count();
    let odd = chars
        .iter()
        .enumerate()
        .filter(|(i, c)| odd(**c, i.checked_sub(1).map(|p| chars[p])))
        .count();
    controls + pairs + odd + foreign_letters(&chars)
}

/// A character no name is expected to hold: past Latin-1, neither a letter nor a digit, nor
/// common punctuation, currency or a letterlike symbol (`’`, `€`, `™`), nor a combining mark on a
/// letter (an NFD accent). Unassigned and private-use code points fall here.
fn odd(c: char, before: Option<char>) -> bool {
    let n = u32::from(c);
    if n < 0x100 || c.is_alphanumeric() {
        return false;
    }
    let common = (0x2000..=0x206f).contains(&n)
        || (0x20a0..=0x20cf).contains(&n)
        || (0x2100..=0x214f).contains(&n);
    let mark_on_letter = (0x300..=0x36f).contains(&n) && before.is_some_and(char::is_alphabetic);
    !(common || mark_on_letter)
}

/// Rough script of a letter: Latin, Greek, Cyrillic, Armenian, Hebrew, Arabic, East Asian, or
/// other.
fn script(c: char) -> Option<u8> {
    if !c.is_alphabetic() {
        return None;
    }
    Some(match u32::from(c) {
        0..=0x24f | 0x1e00..=0x1eff => 0,
        0x370..=0x3ff | 0x1f00..=0x1fff => 1,
        0x400..=0x52f => 2,
        0x530..=0x58f => 3,
        0x590..=0x5ff => 4,
        0x600..=0x6ff | 0x750..=0x77f => 5,
        0x3040..=0x30ff | 0x3400..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff => 6,
        _ => 7,
    })
}

/// Non-ASCII letters not in the script most of the name's letters are in.
fn foreign_letters(chars: &[char]) -> usize {
    let mut per_script = [0usize; 8];
    for c in chars {
        if let Some(s) = script(*c) {
            per_script[usize::from(s)] += 1;
        }
    }
    let main = (0..8u8)
        .max_by_key(|s| (per_script[usize::from(*s)], std::cmp::Reverse(*s)))
        .unwrap_or(0);
    chars
        .iter()
        .filter(|c| !c.is_ascii() && script(**c).is_some_and(|s| s != main))
        .count()
}

/// One round: the name's characters as CP1252/Latin-1 bytes, read as UTF-8.
fn reverse_once(s: &str) -> Option<String> {
    let bytes: Option<Vec<u8>> = s.chars().map(cp1252_byte).collect();
    let bytes = bytes?;
    if bytes.is_ascii() {
        return None;
    }
    String::from_utf8(bytes).ok().filter(|t| t != s)
}

impl Mojibake {
    pub const ID: &'static str = "mojibake";

    /// The repaired name, if this name looks garbled and reversing it helps.
    pub fn propose(&self, entry: &Entry) -> Option<String> {
        let name = entry.name.as_deref()?;
        let mut current = name.to_owned();
        for _ in 0..MAX_ROUNDS {
            match reverse_once(&current) {
                Some(next) if weirdness(&next) < weirdness(&current) => current = next,
                _ => break,
            }
        }
        (current != name).then_some(current)
    }
}

impl NameRule for Mojibake {
    fn id(&self) -> &str {
        Self::ID
    }

    fn applies_to(&self, _target: &TargetProfile) -> bool {
        true
    }

    fn check(&self, entry: &Entry) -> Option<Issue> {
        let after = self.propose(entry)?;
        Some(Issue {
            rule: Self::ID,
            severity: Severity::NeedsDecision,
            message_key: "names.mojibake.reversed",
            before: entry.raw_name.clone(),
            after: after.into_bytes(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> Entry {
        let raw = name.as_bytes().to_vec();
        Entry::new(raw.clone(), raw, 0, false, None)
    }

    fn fix(name: &str) -> Option<String> {
        Mojibake.propose(&file(name))
    }

    #[test]
    fn utf8_read_as_latin1_is_reversed() {
        assert_eq!(fix("PatolÃ³gica.pdf").as_deref(), Some("Patológica.pdf"));
        assert_eq!(fix("AcÃ§Ã£o").as_deref(), Some("Acção"));
        assert_eq!(fix("RelatÃ³rio final").as_deref(), Some("Relatório final"));
    }

    #[test]
    fn utf8_read_as_cp1252_is_reversed() {
        // ’ (e2 80 99) read as CP1252: â € ™
        assert_eq!(fix("Joâ€™s notes").as_deref(), Some("Jo’s notes"));
    }

    #[test]
    fn twice_garbled_is_reversed_twice() {
        let once = "Patológica"
            .bytes()
            .map(|b| crate::encoding::cp1252_char(b).unwrap())
            .collect::<String>();
        let twice = once
            .bytes()
            .map(|b| crate::encoding::cp1252_char(b).unwrap())
            .collect::<String>();
        assert_eq!(fix(&twice).as_deref(), Some("Patológica"));
    }

    #[test]
    fn real_names_are_left_alone() {
        for name in [
            "Patológica.pdf",
            "SÃO PAULO",
            "Ångström",
            "São Tomé e Príncipe",
            "naïve café",
            "plain.txt",
            "日本語.txt",
            "Ü",
        ] {
            assert_eq!(fix(name), None, "{name}");
        }
    }

    #[test]
    fn utf8_cyrillic_read_as_cp1252_is_reversed() {
        assert_eq!(fix("ÐŸÑ€Ð¸Ð²ÐµÑ‚.txt").as_deref(), Some("Привет.txt"));
    }

    #[test]
    fn reversals_into_another_script_or_odd_code_points_are_not_proposed() {
        // Ø + ¨ would read as Arabic ب amid Latin letters; Í + ¸ as the unassigned U+0378.
        assert_eq!(fix("Ø¨ test.txt"), None);
        assert_eq!(fix("Í¸ test.txt"), None);
    }

    #[test]
    fn it_is_a_proposal_not_automatic() {
        let issue = Mojibake.check(&file("PatolÃ³gica")).unwrap();
        assert_eq!(issue.severity, Severity::NeedsDecision);
        assert_eq!(issue.after, "Patológica".as_bytes());
    }

    #[test]
    fn undecodable_bytes_are_not_this_rules_job() {
        let e = Entry::new(b"\xe7a".to_vec(), b"\xe7a".to_vec(), 0, false, None);
        assert_eq!(Mojibake.check(&e), None);
    }
}
