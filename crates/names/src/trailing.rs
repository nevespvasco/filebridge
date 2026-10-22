use crate::{ActiveFor, Entry, Issue, NameRule, Severity, TargetProfile};

/// Removes trailing spaces and dots, which Windows silently strips (so `a.` and `a` collide, and
/// `a.` can't be opened). A name made only of spaces and dots becomes `_`.
///
/// Should be active wherever [`ForbiddenChars`](crate::ForbiddenChars) is: give both the same
/// [`ActiveFor`], as [`general_rules`](crate::general_rules) does.
///
/// Only acts on names that are valid UTF-8; invalid bytes are the `encoding` rule's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trailing {
    active_for: ActiveFor,
}

impl Trailing {
    pub const ID: &'static str = "trailing";
    /// Used when nothing is left after trimming.
    pub const EMPTY_REPLACEMENT: &'static str = "_";

    pub fn new(active_for: ActiveFor) -> Self {
        Self { active_for }
    }

    /// The corrected name, if this rule would change it.
    pub fn propose(&self, entry: &Entry) -> Option<String> {
        let name = entry.name.as_deref()?;
        let trimmed = name.trim_end_matches([' ', '.']);
        if trimmed.len() == name.len() {
            return None;
        }
        if trimmed.is_empty() {
            return Some(Self::EMPTY_REPLACEMENT.to_owned());
        }
        Some(trimmed.to_owned())
    }
}

impl Default for Trailing {
    /// Active on Windows targets only.
    fn default() -> Self {
        Self::new(ActiveFor::windows())
    }
}

impl NameRule for Trailing {
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
            message_key: "names.trailing.removed",
            before: entry.raw_name.clone(),
            after: after.into_bytes(),
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
    fn trailing_dot_is_removed() {
        let e = file(b"notes.");
        let issue = Trailing::default()
            .check(&e)
            .expect("trailing dot must be flagged");
        assert_eq!(issue.rule, "trailing");
        assert_eq!(issue.severity, Severity::Auto);
        assert_eq!(issue.before, b"notes.");
        assert_eq!(issue.after, b"notes");
    }

    #[test]
    fn mixed_trailing_spaces_and_dots_are_removed() {
        let e = file(b"Proposta final . . ");
        assert_eq!(
            Trailing::default().propose(&e).unwrap().as_bytes(),
            b"Proposta final"
        );
    }

    #[test]
    fn leading_and_inner_spaces_and_dots_are_kept() {
        assert_eq!(
            Trailing::default().check(&file(b" .hidden file.tar.gz")),
            None
        );
    }

    #[test]
    fn only_spaces_and_dots_become_underscore() {
        assert_eq!(Trailing::default().propose(&file(b"...")).unwrap(), "_");
        assert_eq!(Trailing::default().propose(&file(b"  ")).unwrap(), "_");
    }

    #[test]
    fn other_whitespace_is_not_trimmed() {
        // NBSP (c2 a0) and tab are not stripped by Windows; tab is forbidden-chars' job.
        assert_eq!(Trailing::default().check(&file(b"a\xc2\xa0")), None);
        assert_eq!(Trailing::default().check(&file(b"a\t")), None);
    }

    #[test]
    fn invalid_utf8_is_not_this_rules_job() {
        assert_eq!(Trailing::default().check(&file(b"\xe7a.")), None);
    }
}
