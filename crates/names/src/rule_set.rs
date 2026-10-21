use crate::{Entry, Issue, NameRule, TargetProfile};

/// Destination path chosen for one entry, plus what was changed on the way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    /// Destination-relative path (`/`-separated, UTF-8). `None` when the name (or an ancestor's)
    /// is not valid UTF-8 and no active rule fixed it.
    pub dest_path: Option<String>,
    /// Final destination name (last component).
    pub dest_name: Option<String>,
    pub issues: Vec<Issue>,
}

impl Mapped {
    pub fn renamed(&self) -> bool {
        !self.issues.is_empty()
    }
}

/// Runs the active rules, in order, over every entry and builds the `source → destination` map.
pub struct RuleSet {
    rules: Vec<Box<dyn NameRule>>,
}

impl RuleSet {
    /// Keeps only the rules that apply to `target`, in the order given.
    pub fn new(rules: Vec<Box<dyn NameRule>>, target: &TargetProfile) -> Self {
        let rules = rules.into_iter().filter(|r| r.applies_to(target)).collect();
        Self { rules }
    }

    /// Maps every entry. `entries` must list each parent before its children (scanner order),
    /// so a corrected directory name is applied to every child's destination path.
    pub fn apply(&self, entries: &[Entry]) -> Vec<Mapped> {
        let mut out: Vec<Mapped> = Vec::with_capacity(entries.len());
        for entry in entries {
            let mut current = entry.clone();
            let mut issues = Vec::new();
            for rule in &self.rules {
                if let Some(issue) = rule.check(&current) {
                    let after = issue.after.clone();
                    issues.push(issue);
                    current.name = String::from_utf8(after.clone()).ok();
                    current.raw_name = after;
                }
            }
            let dest_name = current.name;
            let dest_path = match (entry.parent, &dest_name) {
                (_, None) => None,
                (None, Some(name)) => Some(name.clone()),
                (Some(p), Some(name)) => {
                    assert!(p < out.len(), "parent must come before child");
                    out[p].dest_path.as_ref().map(|pp| format!("{pp}/{name}"))
                }
            };
            out.push(Mapped {
                dest_path,
                dest_name,
                issues,
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Nfc;

    fn rules() -> RuleSet {
        RuleSet::new(vec![Box::new(Nfc)], &TargetProfile::default())
    }

    #[test]
    fn renamed_directory_propagates_to_children() {
        // "Fotografias/Verão/praia.jpg" with "Verão" in NFD (a + cc 83).
        let entries = vec![
            Entry::new(
                b"Fotografias".to_vec(),
                b"Fotografias".to_vec(),
                0,
                true,
                None,
            ),
            Entry::new(
                b"Fotografias/Vera\xcc\x83o".to_vec(),
                b"Vera\xcc\x83o".to_vec(),
                0,
                true,
                Some(0),
            ),
            Entry::new(
                b"Fotografias/Vera\xcc\x83o/praia.jpg".to_vec(),
                b"praia.jpg".to_vec(),
                10,
                false,
                Some(1),
            ),
        ];
        let mapped = rules().apply(&entries);
        assert!(!mapped[0].renamed());
        assert!(mapped[1].renamed());
        assert!(!mapped[2].renamed(), "the child's own name is fine");
        assert_eq!(
            mapped[2].dest_path.as_deref(),
            Some("Fotografias/Ver\u{e3}o/praia.jpg")
        );
        assert_eq!(
            mapped[2].dest_path.as_deref().unwrap().as_bytes(),
            b"Fotografias/Ver\xc3\xa3o/praia.jpg"
        );
    }

    #[test]
    fn invalid_utf8_parent_makes_children_unmappable() {
        let entries = vec![
            Entry::new(b"\xe7a".to_vec(), b"\xe7a".to_vec(), 0, true, None),
            Entry::new(
                b"\xe7a/ok.txt".to_vec(),
                b"ok.txt".to_vec(),
                1,
                false,
                Some(0),
            ),
        ];
        let mapped = rules().apply(&entries);
        assert_eq!(mapped[0].dest_path, None);
        assert_eq!(mapped[1].dest_path, None);
    }
}
