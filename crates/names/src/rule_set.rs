use crate::{Entry, Issue, NameRule, Reserved, TargetProfile};

/// The fixed order rules run in (`docs/04-name-correction.md`). Each rule sees the previous one's
/// proposal, so `collision` sees final names.
pub const RULE_ORDER: [&str; 8] = [
    "encoding",
    "mojibake",
    "nfc",
    "forbidden-chars",
    "trailing",
    "reserved",
    "length",
    "collision",
];

/// Sort key for a rule id. Ids not in [`RULE_ORDER`] (integration-specific rules) run right after
/// `reserved`, so `length` and `collision` still see their output.
fn rank(id: &str) -> usize {
    let position = |id| RULE_ORDER.iter().position(|r| *r == id);
    match position(id) {
        Some(i) => 2 * i,
        None => 2 * position(Reserved::ID).expect("reserved is in RULE_ORDER") + 1,
    }
}

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
    /// Keeps only the rules that apply to `target` and sorts them into [`RULE_ORDER`], whatever
    /// order the caller gives. The sort is stable: rules sharing an id (say the Windows and the
    /// Nextcloud `reserved` lists) and rules with unknown ids keep the caller's order.
    pub fn new(rules: Vec<Box<dyn NameRule>>, target: &TargetProfile) -> Self {
        let mut rules: Vec<_> = rules.into_iter().filter(|r| r.applies_to(target)).collect();
        rules.sort_by_key(|r| rank(r.id()));
        Self { rules }
    }

    /// Ids of the active rules, in the order they run.
    pub fn rule_ids(&self) -> impl Iterator<Item = &str> {
        self.rules.iter().map(|r| r.id())
    }

    /// Maps every entry. `entries` must list each parent before its children (scanner order),
    /// so a corrected directory name is applied to every child's destination path.
    pub fn apply(&self, entries: &[Entry]) -> Vec<Mapped> {
        let mut out: Vec<Mapped> = Vec::with_capacity(entries.len());
        for entry in entries {
            let (dest_name, issues) = self.run(entry, &[]);
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

    /// Runs the chain on one entry's own name, leaving out the rules in `skip`. Returns the
    /// final name (`None` if it is not valid UTF-8) and what each rule changed.
    pub(crate) fn run(&self, entry: &Entry, skip: &[&str]) -> (Option<String>, Vec<Issue>) {
        self.run_where(entry, skip, |_| true)
    }

    /// [`Self::run`], applying only the issues `apply` lets through: the others leave the name
    /// as it was and are not returned.
    pub(crate) fn run_where(
        &self,
        entry: &Entry,
        skip: &[&str],
        apply: impl Fn(&Issue) -> bool,
    ) -> (Option<String>, Vec<Issue>) {
        let mut current = entry.clone();
        let mut issues = Vec::new();
        for rule in self.rules.iter().filter(|r| !skip.contains(&r.id())) {
            if let Some(issue) = rule.check(&current).filter(|i| apply(i)) {
                let after = issue.after.clone();
                issues.push(issue);
                current.name = String::from_utf8(after.clone()).ok();
                current.raw_name = after;
            }
        }
        (current.name, issues)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActiveFor, ForbiddenChars, Nfc, Severity, TargetFamily, Trailing, general_rules};

    fn rules() -> RuleSet {
        RuleSet::new(vec![Box::new(Nfc)], &TargetProfile::default())
    }

    fn windows() -> TargetProfile {
        TargetProfile {
            family: TargetFamily::Windows,
            case_sensitive: false,
            ..TargetProfile::default()
        }
    }

    fn windows_rules() -> RuleSet {
        RuleSet::new(general_rules(ActiveFor::windows()), &windows())
    }

    fn file(raw: &[u8]) -> Entry {
        Entry::new(raw.to_vec(), raw.to_vec(), 0, false, None)
    }

    fn rule_ids(m: &Mapped) -> Vec<&'static str> {
        m.issues.iter().map(|i| i.rule).collect()
    }

    /// An integration-specific rule, to check where unknown ids are ordered.
    struct Custom;

    impl NameRule for Custom {
        fn id(&self) -> &str {
            "custom"
        }
        fn applies_to(&self, _target: &TargetProfile) -> bool {
            true
        }
        fn check(&self, _entry: &Entry) -> Option<Issue> {
            None
        }
    }

    #[test]
    fn rules_are_sorted_into_the_fixed_order() {
        let set = RuleSet::new(
            vec![
                Box::new(Custom),
                Box::new(Reserved::windows()),
                Box::new(Trailing::default()),
                Box::new(ForbiddenChars::default()),
                Box::new(Nfc),
            ],
            &windows(),
        );
        assert_eq!(
            set.rule_ids().collect::<Vec<_>>(),
            ["nfc", "forbidden-chars", "trailing", "reserved", "custom"]
        );
    }

    #[test]
    fn unknown_ids_run_after_reserved() {
        assert!(rank("custom") > rank("reserved"));
        assert!(rank("custom") < rank("length"));
        assert!(rank("custom") < rank("collision"));
    }

    #[test]
    fn rules_that_do_not_apply_are_dropped() {
        let set = RuleSet::new(
            general_rules(ActiveFor::windows()),
            &TargetProfile::default(),
        );
        assert_eq!(
            set.rule_ids().collect::<Vec<_>>(),
            ["encoding", "mojibake", "nfc"]
        );
        let mapped = set.apply(&[file(b"a:b.")]);
        assert!(
            !mapped[0].renamed(),
            "Unix targets accept `:` and a trailing dot"
        );
    }

    #[test]
    fn integration_turns_on_forbidden_chars_and_trailing_but_not_reserved() {
        let nextcloud = TargetProfile {
            integration: Some("nextcloud".into()),
            ..TargetProfile::default()
        };
        let set = RuleSet::new(
            general_rules(ActiveFor::windows().and_integrations(["nextcloud"])),
            &nextcloud,
        );
        assert_eq!(
            set.rule_ids().collect::<Vec<_>>(),
            ["encoding", "mojibake", "nfc", "forbidden-chars", "trailing"]
        );
    }

    #[test]
    fn nfd_name_with_trailing_dot() {
        // "Verão." with ã decomposed (a + cc 83) -> "Verão" composed (c3 a3), no dot.
        let mapped = windows_rules().apply(&[file(b"Vera\xcc\x83o.")]);
        assert_eq!(rule_ids(&mapped[0]), ["nfc", "trailing"]);
        assert_eq!(mapped[0].issues[0].after, b"Ver\xc3\xa3o.");
        assert_eq!(mapped[0].issues[1].before, b"Ver\xc3\xa3o.");
        assert_eq!(
            mapped[0].dest_name.as_deref().unwrap().as_bytes(),
            b"Ver\xc3\xa3o"
        );
        assert!(
            mapped[0]
                .issues
                .iter()
                .all(|i| i.severity == Severity::Auto)
        );
    }

    #[test]
    fn reserved_after_forbidden_char_replacement() {
        // "aux.b|k" -> "aux.b_k" -> still AUX with an extension -> "_aux.b_k".
        let mapped = windows_rules().apply(&[file(b"aux.b|k")]);
        assert_eq!(rule_ids(&mapped[0]), ["forbidden-chars", "reserved"]);
        assert_eq!(mapped[0].dest_name.as_deref(), Some("_aux.b_k"));
    }

    #[test]
    fn reserved_revealed_by_trailing() {
        // Windows would open the device for "NUL . "; trimming exposes it to `reserved`.
        let mapped = windows_rules().apply(&[file(b"NUL . ")]);
        assert_eq!(rule_ids(&mapped[0]), ["trailing", "reserved"]);
        assert_eq!(mapped[0].dest_name.as_deref(), Some("_NUL"));
    }

    #[test]
    fn all_four_rules_on_one_name() {
        // "con.ó?." with ó decomposed -> NFC, `?` replaced, dot trimmed, then CON prefixed.
        let mapped = windows_rules().apply(&[file(b"con.o\xcc\x81?.")]);
        assert_eq!(
            rule_ids(&mapped[0]),
            ["nfc", "forbidden-chars", "trailing", "reserved"]
        );
        assert_eq!(
            mapped[0].dest_name.as_deref().unwrap().as_bytes(),
            b"_con.\xc3\xb3_"
        );
    }

    #[test]
    fn windows_fixes_on_directories_propagate_to_children() {
        // "Q1: rascunho./con.txt" and "Q1: rascunho./AUX/notas.txt".
        let entries = vec![
            Entry::new(
                b"Q1: rascunho.".to_vec(),
                b"Q1: rascunho.".to_vec(),
                0,
                true,
                None,
            ),
            Entry::new(
                b"Q1: rascunho./con.txt".to_vec(),
                b"con.txt".to_vec(),
                3,
                false,
                Some(0),
            ),
            Entry::new(
                b"Q1: rascunho./AUX".to_vec(),
                b"AUX".to_vec(),
                0,
                true,
                Some(0),
            ),
            Entry::new(
                b"Q1: rascunho./AUX/notas.txt".to_vec(),
                b"notas.txt".to_vec(),
                5,
                false,
                Some(2),
            ),
        ];
        let mapped = windows_rules().apply(&entries);
        assert_eq!(rule_ids(&mapped[0]), ["forbidden-chars", "trailing"]);
        assert_eq!(mapped[0].dest_path.as_deref(), Some("Q1_ rascunho"));
        assert_eq!(
            mapped[1].dest_path.as_deref(),
            Some("Q1_ rascunho/_con.txt")
        );
        assert_eq!(mapped[2].dest_path.as_deref(), Some("Q1_ rascunho/_AUX"));
        assert!(!mapped[3].renamed(), "the child's own name is fine");
        assert_eq!(
            mapped[3].dest_path.as_deref(),
            Some("Q1_ rascunho/_AUX/notas.txt")
        );
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
