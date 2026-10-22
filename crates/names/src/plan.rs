//! The Name plan: from scanned entries and the user's decisions to the destination path of every
//! entry. The analysis asks it for proposals ([`NamePlan::propose`]), the decisions step asks
//! whether a choice can be honoured ([`NamePlan::check`]), and Send asks for the final paths
//! ([`NamePlan::resolve`]). The same rules run in all three, so a name the user keeps or types
//! is held to the rules the analysis applied.

use std::collections::{HashMap, HashSet};
use std::fmt;

use unicode_normalization::UnicodeNormalization;

use crate::{Entry, EntryId, Issue, Mapped, RuleSet, Severity, TargetProfile};

/// Two entries would land on the same destination path.
pub const RULE_COLLISION: &str = "collision";
/// A name that is not valid UTF-8: decoded by the `encoding` rule, or, when that rule is off, an
/// issue with no proposal.
pub const RULE_ENCODING: &str = "encoding";
/// A name, or the path it ends, longer than the destination takes.
pub const RULE_LENGTH: &str = "length";

/// The shortest name `length` proposes, whatever the path: below this the name says nothing.
const MIN_NAME_BYTES: usize = 16;
/// The marker put where `length` cuts a name.
const CUT: &str = "…";

/// What the user chose for one issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Use the rule's proposal.
    Accept,
    /// Don't apply this rule to this entry. The other rules still apply.
    KeepOriginal,
    /// Use this name for the entry instead. It must pass every active rule.
    Edit(String),
    /// Leave the entry (and everything under it) out of the send.
    Skip,
}

/// One issue of the analysis and the user's choice for it (`None` while pending).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decided {
    pub entry: EntryId,
    pub rule: String,
    /// The rule's proposal as the analysis found it; what accepting a collision sends.
    pub after: Vec<u8>,
    pub choice: Option<Choice>,
}

/// Where one entry goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Planned {
    Path(String),
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    /// This many issues still wait for a decision.
    Pending(u64),
    /// Accept on a name that has no proposal (an undecodable one).
    NoProposal { path: String },
    /// The name is not valid UTF-8; it has to be edited or skipped.
    Undecodable { path: String },
    /// Empty, `.`, `..`, or containing `/` or NUL.
    Invalid { name: String },
    /// A name the user kept or typed that an active rule would still change.
    RuleStillApplies {
        path: String,
        rule: String,
        proposal: String,
    },
    /// Two entries would be sent to the same path.
    Duplicate { path: String },
    /// Entries out of scanner order.
    ParentAfterChild { entry: EntryId },
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending(n) => write!(f, "{n} name decisions are still pending"),
            Self::NoProposal { path } => {
                write!(
                    f,
                    "{path}: an undecodable name has no proposal; edit or skip it"
                )
            }
            Self::Undecodable { path } => {
                write!(
                    f,
                    "{path}: the original name is not valid UTF-8; edit or skip it"
                )
            }
            Self::Invalid { name } => write!(f, "invalid destination name {name:?}"),
            Self::RuleStillApplies {
                path,
                rule,
                proposal,
            } => write!(
                f,
                "{path}: the rule {rule} would still change this name (to {proposal:?}); \
                 pick another name or skip the file"
            ),
            Self::Duplicate { path } => {
                write!(
                    f,
                    "two entries would be sent to {path:?}; rename or skip one"
                )
            }
            Self::ParentAfterChild { entry } => write!(f, "entry {entry}: parent after child"),
        }
    }
}

impl std::error::Error for NameError {}

/// The active rules of one destination, whether it tells names apart by case, and the longest
/// name and path it takes.
pub struct NamePlan {
    rules: RuleSet,
    case_sensitive: bool,
    max_name_bytes: usize,
    max_path_bytes: usize,
}

impl NamePlan {
    /// With the default limits (255-byte names, 4096-byte paths).
    pub fn new(rules: RuleSet, case_sensitive: bool) -> Self {
        let target = TargetProfile::default();
        Self {
            rules,
            case_sensitive,
            max_name_bytes: target.max_name_bytes,
            max_path_bytes: target.max_path_bytes,
        }
    }

    /// With `target`'s case sensitivity and limits.
    pub fn for_target(rules: RuleSet, target: &TargetProfile) -> Self {
        Self {
            rules,
            case_sensitive: target.case_sensitive,
            max_name_bytes: target.max_name_bytes,
            max_path_bytes: target.max_path_bytes,
        }
    }

    /// Ids of the active rules, in the order they run.
    pub fn rule_ids(&self) -> impl Iterator<Item = &str> {
        self.rules.rule_ids()
    }

    /// Every entry's proposed destination path and its issues: the rules' own, an `encoding`
    /// issue for a name no rule could decode, a `length` issue (proposing a name cut in the
    /// middle) for a name or path longer than the destination takes, and a `collision` issue
    /// (proposing a ` (2)` suffix) for a path an earlier entry already takes. `entries` in
    /// scanner order.
    pub fn propose(&self, entries: &[Entry]) -> Vec<Mapped> {
        let mut mapped = self.rules.apply(entries);
        let mut taken = HashSet::new();
        for (entry, m) in entries.iter().zip(&mut mapped) {
            let (Some(path), Some(name)) = (m.dest_path.clone(), m.dest_name.clone()) else {
                if m.dest_name.is_none() {
                    m.issues.push(Issue {
                        rule: RULE_ENCODING,
                        severity: Severity::NeedsDecision,
                        message_key: "names.encoding.undecodable",
                        before: entry.raw_name.clone(),
                        after: entry.raw_name.clone(),
                    });
                }
                continue; // an ancestor is undecodable; its issue covers this entry
            };
            let parent = path.rsplit_once('/').map(|(p, _)| p.to_owned());
            let join = |n: &str| {
                parent
                    .as_deref()
                    .map_or(n.to_owned(), |pp| format!("{pp}/{n}"))
            };
            let mut name = name;
            if let Some((short, key)) = self.fit(parent.as_deref(), &name) {
                m.issues.push(Issue {
                    rule: RULE_LENGTH,
                    severity: Severity::NeedsDecision,
                    message_key: key,
                    before: name.as_bytes().to_vec(),
                    after: short.as_bytes().to_vec(),
                });
                name = short;
            }
            if taken.insert(self.key(&join(&name))) {
                continue;
            }
            let proposal = (2..)
                .map(|n| with_suffix(&name, n))
                .find(|cand| !taken.contains(&self.key(&join(cand))))
                .expect("an unused suffix exists");
            taken.insert(self.key(&join(&proposal)));
            m.issues.push(Issue {
                rule: RULE_COLLISION,
                severity: Severity::NeedsDecision,
                message_key: "names.collision",
                before: name.into_bytes(),
                after: proposal.into_bytes(),
            });
        }
        mapped
    }

    /// A shorter `name` (and the message for it) when it, or the path it ends under `parent`,
    /// is longer than the destination takes.
    fn fit(&self, parent: Option<&str>, name: &str) -> Option<(String, &'static str)> {
        let room = parent.map_or(usize::MAX, |p| {
            self.max_path_bytes.saturating_sub(p.len() + 1)
        });
        let (limit, key) = if name.len() > self.max_name_bytes && self.max_name_bytes <= room {
            (self.max_name_bytes, "names.length.name")
        } else if name.len() > room {
            (room.max(MIN_NAME_BYTES), "names.length.path")
        } else {
            return None;
        };
        let short = shorten(name, limit);
        (short != name).then_some((short, key))
    }

    /// Whether `choice` on the `rule` issue of `entry` can be honoured. Called when the user
    /// decides, so a choice that can't work is refused then and not at Send. Whether two
    /// entries end up on the same path is only known at [`NamePlan::resolve`].
    pub fn check(&self, entry: &Entry, rule: &str, choice: &Choice) -> Result<(), NameError> {
        match choice {
            Choice::Skip => Ok(()),
            Choice::Accept if rule == RULE_ENCODING => self.decodable(entry),
            Choice::Accept => Ok(()),
            Choice::Edit(name) => self.check_edit(entry, name),
            Choice::KeepOriginal if rule == RULE_COLLISION || rule == RULE_LENGTH => Ok(()),
            Choice::KeepOriginal => self.kept_name(entry, &[rule]).map(drop),
        }
    }

    /// The final path, or skip, of every entry: the rules' result with the user's decisions
    /// applied and re-checked. `entries` in scanner order; a skipped directory skips everything
    /// under it, and a renamed one moves everything under it.
    pub fn resolve(
        &self,
        entries: &[Entry],
        decisions: &[Decided],
    ) -> Result<Vec<Planned>, NameError> {
        let pending = decisions.iter().filter(|d| d.choice.is_none()).count();
        if pending > 0 {
            return Err(NameError::Pending(pending as u64));
        }
        let mut by_entry: HashMap<EntryId, Vec<&Decided>> = HashMap::new();
        for d in decisions {
            by_entry.entry(d.entry).or_default().push(d);
        }
        let mut out: Vec<Planned> = Vec::with_capacity(entries.len());
        let mut seen = HashSet::new();
        for (i, entry) in entries.iter().enumerate() {
            let parent = match entry.parent {
                None => None,
                Some(p) if p >= i => return Err(NameError::ParentAfterChild { entry: i }),
                Some(p) => match &out[p] {
                    Planned::Skipped => {
                        out.push(Planned::Skipped);
                        continue;
                    }
                    Planned::Path(pp) => Some(pp.clone()),
                },
            };
            let decided = by_entry.get(&i).map_or(&[][..], Vec::as_slice);
            let Some(name) = self.final_name(entry, decided)? else {
                out.push(Planned::Skipped);
                continue;
            };
            let path = parent.map_or(name.clone(), |pp| format!("{pp}/{name}"));
            if !seen.insert(self.key(&path)) {
                return Err(NameError::Duplicate { path });
            }
            out.push(Planned::Path(path));
        }
        Ok(out)
    }

    /// One entry's own final name, or `None` to skip it. `decided` has no pending choice.
    fn final_name(&self, entry: &Entry, decided: &[&Decided]) -> Result<Option<String>, NameError> {
        let choices = || {
            decided
                .iter()
                .filter_map(|d| d.choice.as_ref().map(|c| (d, c)))
        };
        if choices().any(|(_, c)| *c == Choice::Skip) {
            return Ok(None);
        }
        if let Some((_, Choice::Edit(name))) =
            choices().rev().find(|(_, c)| matches!(c, Choice::Edit(_)))
        {
            self.check_edit(entry, name)?;
            return Ok(Some(name.clone()));
        }
        if choices().any(|(d, c)| d.rule == RULE_ENCODING && *c == Choice::Accept) {
            self.decodable(entry)?;
        }
        let whole_tree = |rule: &str| rule == RULE_COLLISION || rule == RULE_LENGTH;
        let kept: Vec<&str> = choices()
            .filter(|(d, c)| **c == Choice::KeepOriginal && !whole_tree(&d.rule))
            .map(|(d, _)| d.rule.as_str())
            .collect();
        let mut name = self.kept_name(entry, &kept)?;
        // A collision's proposal already carries the length cut, so it wins over it.
        let accepted =
            |rule: &str| choices().find(|(d, c)| d.rule == rule && **c == Choice::Accept);
        if let Some((d, _)) = accepted(RULE_COLLISION).or_else(|| accepted(RULE_LENGTH)) {
            name = String::from_utf8_lossy(&d.after).into_owned();
        }
        valid(&name)?;
        Ok(Some(name))
    }

    /// A name the user typed: well formed, and left alone by every active rule.
    fn check_edit(&self, entry: &Entry, name: &str) -> Result<(), NameError> {
        valid(name)?;
        let typed = Entry {
            raw_name: name.as_bytes().to_vec(),
            name: Some(name.to_owned()),
            ..entry.clone()
        };
        match self.rules.run(&typed, &[]) {
            (_, issues) if issues.is_empty() => Ok(()),
            (after, issues) => Err(NameError::RuleStillApplies {
                path: display(entry),
                rule: issues[0].rule.to_owned(),
                proposal: after.unwrap_or_default(),
            }),
        }
    }

    /// The entry's name with the rules in `kept` left out. Refused when that name is not valid
    /// UTF-8, or when leaving them out makes another rule change it instead (say Nextcloud's own
    /// NFC rule, once the general one is kept out).
    fn kept_name(&self, entry: &Entry, kept: &[&str]) -> Result<String, NameError> {
        if kept.contains(&RULE_ENCODING) {
            return Err(NameError::Undecodable {
                path: display(entry),
            });
        }
        let (name, issues) = self.rules.run(entry, kept);
        let name = name.ok_or_else(|| NameError::Undecodable {
            path: display(entry),
        })?;
        if !kept.is_empty() {
            let (_, analysed) = self.rules.run(entry, &[]);
            if let Some(new) = issues
                .iter()
                .find(|i| !analysed.iter().any(|a| a.rule == i.rule))
            {
                return Err(NameError::RuleStillApplies {
                    path: display(entry),
                    rule: new.rule.to_owned(),
                    proposal: name,
                });
            }
        }
        Ok(name)
    }

    /// Accept on `encoding` needs a reading of the name: there is none when the `encoding` rule
    /// is off and the name is not UTF-8.
    fn decodable(&self, entry: &Entry) -> Result<(), NameError> {
        match self.rules.run(entry, &[]) {
            (Some(_), _) => Ok(()),
            (None, _) => Err(NameError::NoProposal {
                path: display(entry),
            }),
        }
    }

    /// What two paths must not share: their NFC form (servers that normalise would make them
    /// one), and, on a destination that ignores case, their lowercase form.
    fn key(&self, path: &str) -> String {
        let nfc: String = path.nfc().collect();
        if self.case_sensitive {
            nfc
        } else {
            nfc.to_lowercase()
        }
    }
}

fn valid(name: &str) -> Result<(), NameError> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(NameError::Invalid {
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn display(entry: &Entry) -> String {
    String::from_utf8_lossy(&entry.raw_path).into_owned()
}

/// `name` cut to at most `max` bytes in the middle of its stem, keeping the extension and both
/// ends: `Relatório anual de atividades.pdf` → `Relatório a…tividades.pdf`. Always on character
/// boundaries; a name that fits is returned as is.
pub fn shorten(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= 16 => name.split_at(dot),
        _ => (name, ""),
    };
    let floor = |s: &str, n: usize| (0..=n.min(s.len())).rev().find(|i| s.is_char_boundary(*i));
    let Some(room) = max.checked_sub(ext.len() + CUT.len()).filter(|r| *r >= 2) else {
        let end = floor(name, max).unwrap_or(0);
        return name[..end].to_owned();
    };
    let head = floor(stem, room / 2).unwrap_or(0);
    let tail_len = room - head;
    let tail = (stem.len().saturating_sub(tail_len)..=stem.len())
        .find(|i| stem.is_char_boundary(*i))
        .unwrap_or(stem.len());
    format!("{}{CUT}{}{ext}", &stem[..head], &stem[tail..])
}

/// `photo.jpg` → `photo (2).jpg`; `.bashrc` and `README` get the suffix at the end.
pub fn with_suffix(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActiveFor, NameRule, Nfc, TargetFamily, TargetProfile, general_rules};

    fn nfc_only(case_sensitive: bool) -> NamePlan {
        NamePlan::new(
            RuleSet::new(vec![Box::new(Nfc)], &TargetProfile::default()),
            case_sensitive,
        )
    }

    fn windows() -> NamePlan {
        let target = TargetProfile {
            family: TargetFamily::Windows,
            case_sensitive: false,
            ..TargetProfile::default()
        };
        NamePlan::new(
            RuleSet::new(general_rules(ActiveFor::windows()), &target),
            false,
        )
    }

    /// Stands in for an integration's own NFC rule (Nextcloud's), which fires only when the
    /// general one is kept out.
    struct StrictNfc;

    impl NameRule for StrictNfc {
        fn id(&self) -> &str {
            "strict-nfc"
        }
        fn applies_to(&self, _target: &TargetProfile) -> bool {
            true
        }
        fn check(&self, entry: &Entry) -> Option<Issue> {
            Some(Issue {
                rule: "strict-nfc",
                ..Nfc.check(entry)?
            })
        }
    }

    fn strict() -> NamePlan {
        NamePlan::new(
            RuleSet::new(
                vec![Box::new(Nfc), Box::new(StrictNfc)],
                &TargetProfile::default(),
            ),
            true,
        )
    }

    /// Entries from `(parent, path)`, in that order.
    fn tree(items: &[(Option<usize>, &[u8])]) -> Vec<Entry> {
        items
            .iter()
            .map(|(parent, path)| {
                let name = path.rsplit(|b| *b == b'/').next().unwrap().to_vec();
                Entry::new(path.to_vec(), name, 0, false, *parent)
            })
            .collect()
    }

    /// Every issue of `propose`, decided with the analysis' defaults: automatic ones accepted.
    fn defaults(mapped: &[Mapped]) -> Vec<Decided> {
        let mut out = Vec::new();
        for (i, m) in mapped.iter().enumerate() {
            for issue in &m.issues {
                out.push(Decided {
                    entry: i,
                    rule: issue.rule.into(),
                    after: issue.after.clone(),
                    choice: (issue.severity == Severity::Auto).then_some(Choice::Accept),
                });
            }
        }
        out
    }

    fn set(decisions: &mut [Decided], entry: EntryId, rule: &str, choice: Choice) {
        let d = decisions
            .iter_mut()
            .find(|d| d.entry == entry && d.rule == rule)
            .expect("issue exists");
        d.choice = Some(choice);
    }

    fn path(p: &str) -> Planned {
        Planned::Path(p.into())
    }

    #[test]
    fn suffixes() {
        assert_eq!(with_suffix("photo.jpg", 2), "photo (2).jpg");
        assert_eq!(with_suffix(".bashrc", 2), ".bashrc (2)");
        assert_eq!(with_suffix("README", 3), "README (3)");
    }

    #[test]
    fn nfd_and_nfc_twins_collide_and_accepting_takes_the_suffix() {
        // "Verão.txt" twice: NFC and NFD. After NFC both map to the same name.
        let entries = tree(&[
            (None, b"Acme"),
            (Some(0), "Acme/Ver\u{e3}o.txt".as_bytes()),
            (Some(0), "Acme/Vera\u{303}o.txt".as_bytes()),
        ]);
        let plan = nfc_only(true);
        let mapped = plan.propose(&entries);
        assert_eq!(mapped[2].issues[0].rule, "nfc");
        assert_eq!(mapped[2].issues[1].rule, RULE_COLLISION);
        assert_eq!(mapped[2].issues[1].after, "Verão (2).txt".as_bytes());

        let mut decisions = defaults(&mapped);
        assert_eq!(
            plan.resolve(&entries, &decisions),
            Err(NameError::Pending(1))
        );
        set(&mut decisions, 2, RULE_COLLISION, Choice::Accept);
        let planned = plan.resolve(&entries, &decisions).unwrap();
        assert_eq!(planned[2], path("Acme/Verão (2).txt"));

        set(&mut decisions, 2, RULE_COLLISION, Choice::KeepOriginal);
        assert!(matches!(
            plan.resolve(&entries, &decisions),
            Err(NameError::Duplicate { .. })
        ));
    }

    #[test]
    fn case_insensitive_targets_collide_on_case() {
        let entries = tree(&[(None, b"d"), (Some(0), b"d/A.txt"), (Some(0), b"d/a.txt")]);
        let collisions = |plan: NamePlan| {
            plan.propose(&entries)
                .iter()
                .flat_map(|m| &m.issues)
                .filter(|i| i.rule == RULE_COLLISION)
                .count()
        };
        assert_eq!(collisions(nfc_only(false)), 1);
        assert_eq!(collisions(nfc_only(true)), 0);
    }

    #[test]
    fn undecodable_names_need_a_decision_and_have_nothing_to_accept() {
        let entries = tree(&[(None, b"Patol\xf3gica.pdf")]);
        let plan = nfc_only(true);
        let mapped = plan.propose(&entries);
        assert_eq!(mapped[0].issues[0].rule, RULE_ENCODING);
        assert_eq!(mapped[0].issues[0].severity, Severity::NeedsDecision);

        for choice in [Choice::Accept, Choice::KeepOriginal] {
            assert!(plan.check(&entries[0], RULE_ENCODING, &choice).is_err());
        }
        assert_eq!(
            plan.check(&entries[0], RULE_ENCODING, &Choice::Skip),
            Ok(())
        );
        let edit = Choice::Edit("Patológica.pdf".into());
        assert_eq!(plan.check(&entries[0], RULE_ENCODING, &edit), Ok(()));
    }

    #[test]
    fn editing_an_undecodable_folder_maps_its_children() {
        let entries = tree(&[(None, b"\xe7a"), (Some(0), b"\xe7a/ok.txt")]);
        let plan = nfc_only(true);
        let mapped = plan.propose(&entries);
        assert_eq!(mapped[1].dest_path, None);
        assert!(mapped[1].issues.is_empty(), "the folder's issue covers it");

        let mut decisions = defaults(&mapped);
        set(&mut decisions, 0, RULE_ENCODING, Choice::Edit("ça".into()));
        let planned = plan.resolve(&entries, &decisions).unwrap();
        assert_eq!(planned, [path("ça"), path("ça/ok.txt")]);
    }

    #[test]
    fn keeping_one_rule_out_keeps_the_others() {
        // "Verão." in NFD on Windows: `nfc`, then `trailing`. Keeping the original for `nfc`
        // leaves the name decomposed but still drops the trailing dot.
        let entries = tree(&[(None, "Vera\u{303}o.".as_bytes())]);
        let plan = windows();
        let mut decisions = defaults(&plan.propose(&entries));
        set(&mut decisions, 0, "nfc", Choice::KeepOriginal);
        let planned = plan.resolve(&entries, &decisions).unwrap();
        assert_eq!(planned, [path("Vera\u{303}o")]);
    }

    #[test]
    fn keeping_the_original_is_refused_when_another_rule_takes_over() {
        let entries = tree(&[(None, "Vera\u{303}o".as_bytes())]);
        let plan = strict();
        let mapped = plan.propose(&entries);
        assert_eq!(mapped[0].issues.len(), 1, "only the general rule fires");

        let err = plan
            .check(&entries[0], "nfc", &Choice::KeepOriginal)
            .unwrap_err();
        assert!(matches!(&err, NameError::RuleStillApplies { rule, .. } if rule == "strict-nfc"));

        let mut decisions = defaults(&mapped);
        set(&mut decisions, 0, "nfc", Choice::KeepOriginal);
        assert_eq!(plan.resolve(&entries, &decisions), Err(err));
    }

    #[test]
    fn edited_names_must_pass_every_rule() {
        let entries = tree(&[(None, b"a:b")]);
        let plan = windows();
        let check =
            |name: &str| plan.check(&entries[0], "forbidden-chars", &Choice::Edit(name.into()));
        assert_eq!(check("a-b"), Ok(()));
        assert!(
            matches!(check("CON"), Err(NameError::RuleStillApplies { rule, .. }) if rule == "reserved")
        );
        assert!(
            matches!(check("x."), Err(NameError::RuleStillApplies { rule, .. }) if rule == "trailing")
        );
        assert!(matches!(check("a/b"), Err(NameError::Invalid { .. })));
        assert!(matches!(check(".."), Err(NameError::Invalid { .. })));

        let mut decisions = defaults(&plan.propose(&entries));
        set(
            &mut decisions,
            0,
            "forbidden-chars",
            Choice::Edit("CON".into()),
        );
        assert!(
            plan.resolve(&entries, &decisions).is_err(),
            "Send re-checks too"
        );
    }

    #[test]
    fn skip_and_edit_reach_the_children() {
        let entries = tree(&[
            (None, b"Acme"),
            (Some(0), "Acme/Vera\u{303}o".as_bytes()),
            (Some(1), "Acme/Vera\u{303}o/a.txt".as_bytes()),
        ]);
        let plan = nfc_only(true);
        let mapped = plan.propose(&entries);

        let mut decisions = defaults(&mapped);
        set(&mut decisions, 1, "nfc", Choice::Edit("Pasta".into()));
        let planned = plan.resolve(&entries, &decisions).unwrap();
        assert_eq!(planned[2], path("Acme/Pasta/a.txt"));

        set(&mut decisions, 1, "nfc", Choice::Skip);
        let planned = plan.resolve(&entries, &decisions).unwrap();
        assert_eq!(planned[1..], [Planned::Skipped, Planned::Skipped]);
    }

    #[test]
    fn a_name_without_issues_is_sent_as_the_rules_leave_it() {
        let entries = tree(&[(None, b"Acme"), (Some(0), b"Acme/b.txt")]);
        let plan = nfc_only(true);
        let planned = plan.resolve(&entries, &[]).unwrap();
        assert_eq!(planned, [path("Acme"), path("Acme/b.txt")]);
    }

    fn limited(max_name: usize, max_path: usize) -> NamePlan {
        let target = TargetProfile {
            max_name_bytes: max_name,
            max_path_bytes: max_path,
            ..TargetProfile::default()
        };
        NamePlan::for_target(RuleSet::new(vec![Box::new(Nfc)], &target), &target)
    }

    #[test]
    fn shorten_cuts_the_middle_and_keeps_the_extension() {
        let long = "Relatório anual de atividades.pdf";
        let short = shorten(long, 24);
        assert!(short.len() <= 24, "{short}");
        assert!(
            short.starts_with("Rela") && short.ends_with("des.pdf"),
            "{short}"
        );
        assert!(short.contains(CUT));
        assert_eq!(shorten("ok.txt", 24), "ok.txt");
        // Multi-byte characters are never split.
        let s = shorten(&"ção".repeat(40), 31);
        assert!(s.len() <= 31 && s.is_char_boundary(s.len()));
    }

    #[test]
    fn a_name_too_long_for_the_destination_is_a_decision() {
        let entries = tree(&[(None, "a".repeat(40).as_bytes())]);
        let plan = limited(20, 4096);
        let mapped = plan.propose(&entries);
        let issue = &mapped[0].issues[0];
        assert_eq!(issue.rule, RULE_LENGTH);
        assert_eq!(issue.severity, Severity::NeedsDecision);
        assert_eq!(issue.message_key, "names.length.name");
        assert!(issue.after.len() <= 20);

        let mut decisions = defaults(&mapped);
        assert!(plan.resolve(&entries, &decisions).is_err(), "pending");
        set(&mut decisions, 0, RULE_LENGTH, Choice::Accept);
        let short = String::from_utf8(issue.after.clone()).unwrap();
        assert_eq!(plan.resolve(&entries, &decisions).unwrap(), [path(&short)]);
        set(&mut decisions, 0, RULE_LENGTH, Choice::KeepOriginal);
        assert_eq!(
            plan.resolve(&entries, &decisions).unwrap(),
            [path(&"a".repeat(40))]
        );
    }

    #[test]
    fn a_path_too_long_shortens_its_last_name() {
        let deep = format!("{}/{}", "d".repeat(30), "f".repeat(30));
        let entries = tree(&[
            (None, "d".repeat(30).as_bytes()),
            (Some(0), deep.as_bytes()),
        ]);
        let mapped = limited(255, 50).propose(&entries);
        assert!(mapped[0].issues.is_empty(), "the folder fits");
        let issue = &mapped[1].issues[0];
        assert_eq!(issue.message_key, "names.length.path");
        assert!(30 + 1 + issue.after.len() <= 50);
    }

    #[test]
    fn nfc_and_nfd_spellings_collide() {
        // "Verão" twice: once NFD (a + cc 83), once NFC (c3 a3). Without the nfc rule both would
        // reach a normalising server as the same name.
        let entries = tree(&[(None, b"Vera\xcc\x83o"), (None, b"Ver\xc3\xa3o")]);
        let plan = NamePlan::new(RuleSet::new(vec![], &TargetProfile::default()), true);
        let mapped = plan.propose(&entries);
        assert!(mapped[0].issues.is_empty());
        assert_eq!(mapped[1].issues[0].rule, RULE_COLLISION);
    }

    #[test]
    fn a_latin1_name_is_decoded_and_can_be_accepted() {
        // "Acção" in Latin-1, under the general rules.
        let entries = tree(&[(None, b"Ac\xe7\xe3o")]);
        let plan = NamePlan::new(
            RuleSet::new(
                general_rules(ActiveFor::windows()),
                &TargetProfile::default(),
            ),
            true,
        );
        let mapped = plan.propose(&entries);
        assert_eq!(mapped[0].issues[0].rule, RULE_ENCODING);
        assert_eq!(mapped[0].dest_path.as_deref(), Some("Acção"));
        assert_eq!(
            plan.check(&entries[0], RULE_ENCODING, &Choice::Accept),
            Ok(())
        );
        let decisions = defaults(&mapped);
        assert_eq!(plan.resolve(&entries, &decisions).unwrap(), [path("Acção")]);
    }
}
