//! The Name plan: from scanned entries and the user's decisions to the destination path of every
//! entry. The analysis asks it for proposals ([`NamePlan::propose`]), the decisions step asks
//! whether a choice can be honoured ([`NamePlan::check`]), and Send asks for the final paths
//! ([`NamePlan::resolve`]). The same rules run in all three, so a name the user keeps or types
//! is held to the rules the analysis applied.

use std::collections::{HashMap, HashSet};
use std::fmt;

use unicode_normalization::UnicodeNormalization;

use crate::{Entry, EntryId, Issue, Mapped, RuleSet, Severity, TargetFamily, TargetProfile};

/// Two entries would land on the same destination path.
pub const RULE_COLLISION: &str = "collision";
/// A name that is not valid UTF-8: decoded by the `encoding` rule, or, when that rule is off, an
/// issue with no proposal.
pub const RULE_ENCODING: &str = "encoding";
/// A name, or the path it ends, longer than the destination takes.
pub const RULE_LENGTH: &str = "length";

/// The shortest name `length` proposes, whatever the path: below this the name says nothing.
/// Also the smallest name limit a destination can have.
pub const MIN_NAME_LEN: usize = 16;
/// What a folder cut for its path leaves free for the names inside it.
const DIR_HEADROOM: usize = 2 * MIN_NAME_LEN;
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
    /// The name before the rule, as the analysis found it.
    pub before: Vec<u8>,
    /// The rule's proposal as the analysis found it; equal to `before` when there is none. For
    /// a collision, its ` (n)` is the number accepting it sends.
    pub after: Vec<u8>,
    pub choice: Option<Choice>,
}

impl Decided {
    /// Whether the rule proposed a name (an undecodable name, or one no cut can fit, has none).
    pub fn has_proposal(&self) -> bool {
        self.after != self.before
    }
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
    /// Accept on an issue that has no proposal (an undecodable name, a path no cut fits).
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
    /// A name, or the path it ends, longer than the destination takes, that the user didn't
    /// choose to keep.
    TooLong { path: String },
    /// Entries out of scanner order.
    ParentAfterChild { entry: EntryId },
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pending(n) => write!(f, "{n} name decisions are still pending"),
            Self::NoProposal { path } => {
                write!(f, "{path}: there is no proposal to accept; edit or skip it")
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
            Self::TooLong { path } => write!(
                f,
                "{path}: the name, or the path it ends, is longer than the destination takes; \
                 shorten it or skip it"
            ),
            Self::ParentAfterChild { entry } => write!(f, "entry {entry}: parent after child"),
        }
    }
}

impl std::error::Error for NameError {}

/// Whether a name fits where it goes, and the cut that makes it fit.
enum Fit {
    Fits,
    Cut(String, &'static str),
    /// Even the shortest cut wouldn't fit: the path above it already takes the room.
    NoRoom(&'static str),
}

/// The active rules of one destination, whether it tells names apart by case, and the longest
/// name and path it takes.
pub struct NamePlan {
    rules: RuleSet,
    case_sensitive: bool,
    /// Lengths in UTF-16 units (Windows) instead of UTF-8 bytes.
    utf16: bool,
    max_name: usize,
    max_path: usize,
    /// Length of the folder the paths land under, with its `/`; 0 at the destination root.
    base: usize,
}

impl NamePlan {
    /// With the default limits (255-byte names, 4096-byte paths).
    pub fn new(rules: RuleSet, case_sensitive: bool) -> Self {
        let target = TargetProfile {
            case_sensitive,
            ..TargetProfile::default()
        };
        Self::for_target(rules, &target)
    }

    /// With `target`'s case sensitivity and limits, measured in UTF-16 units on Windows and in
    /// UTF-8 bytes elsewhere. Limits below [`MIN_NAME_LEN`] are raised to it.
    pub fn for_target(rules: RuleSet, target: &TargetProfile) -> Self {
        Self {
            rules,
            case_sensitive: target.case_sensitive,
            utf16: target.family == TargetFamily::Windows,
            max_name: target.max_name_bytes.max(MIN_NAME_LEN),
            max_path: target.max_path_bytes.max(MIN_NAME_LEN),
            base: 0,
        }
    }

    /// The paths land under `dir` (a send's target folder, the folder a repair works in), so
    /// its length counts against the path limit.
    pub fn under(mut self, dir: Option<&str>) -> Self {
        self.base = dir
            .map(|d| d.trim_matches('/'))
            .filter(|d| !d.is_empty())
            .map_or(0, |d| self.len(d) + 1);
        self
    }

    /// Ids of the active rules, in the order they run.
    pub fn rule_ids(&self) -> impl Iterator<Item = &str> {
        self.rules.rule_ids()
    }

    /// Every entry's proposed destination path and its issues: the rules' own, an `encoding`
    /// issue for a name no rule could decode, a `length` issue (proposing a name cut in the
    /// middle, or nothing when no cut fits) for a name or path longer than the destination
    /// takes, and a `collision` issue (proposing a ` (2)` suffix) for a path an earlier entry
    /// already takes. Each path is measured under its parent's proposed path, and the proposed
    /// paths assume every proposal accepted. `entries` in scanner order.
    pub fn propose(&self, entries: &[Entry]) -> Vec<Mapped> {
        let mut mapped = self.rules.apply(entries);
        let mut taken = HashSet::new();
        let mut paths: Vec<Option<String>> = Vec::with_capacity(entries.len());
        for (entry, m) in entries.iter().zip(&mut mapped) {
            let parent = match entry.parent {
                None => Some(None),
                Some(p) => paths[p].clone().map(Some),
            };
            let (Some(parent), Some(name)) = (parent, m.dest_name.clone()) else {
                if m.dest_name.is_none() {
                    m.issues.push(Issue {
                        rule: RULE_ENCODING,
                        severity: Severity::NeedsDecision,
                        message_key: "names.encoding.undecodable",
                        before: entry.raw_name.clone(),
                        after: entry.raw_name.clone(),
                    });
                }
                // An undecodable ancestor's issue covers this entry.
                m.dest_path = None;
                paths.push(None);
                continue;
            };
            let parent = parent.as_deref();
            let mut name = name;
            let length = |after: &str, key| Issue {
                rule: RULE_LENGTH,
                severity: Severity::NeedsDecision,
                message_key: key,
                before: name.as_bytes().to_vec(),
                after: after.as_bytes().to_vec(),
            };
            match self.fit(parent, &name, entry.is_dir) {
                Fit::Fits => {}
                Fit::Cut(short, key) => {
                    m.issues.push(length(&short, key));
                    name = short;
                }
                Fit::NoRoom(key) => m.issues.push(length(&name, key)),
            }
            if !taken.insert(self.key(&join(parent, &name))) {
                let proposal = (2..)
                    .map(|n| self.suffixed(&name, n, parent, true))
                    .find(|cand| !taken.contains(&self.key(&join(parent, cand))))
                    .expect("an unused suffix exists");
                taken.insert(self.key(&join(parent, &proposal)));
                m.issues.push(Issue {
                    rule: RULE_COLLISION,
                    severity: Severity::NeedsDecision,
                    message_key: "names.collision",
                    before: name.into_bytes(),
                    after: proposal.as_bytes().to_vec(),
                });
                name = proposal;
            }
            let path = join(parent, &name);
            m.dest_name = Some(name);
            m.dest_path = Some(path.clone());
            paths.push(Some(path));
        }
        mapped
    }

    /// `s`'s length the way the destination measures it.
    fn len(&self, s: &str) -> usize {
        if self.utf16 {
            s.encode_utf16().count()
        } else {
            s.len()
        }
    }

    fn width(&self) -> fn(char) -> usize {
        if self.utf16 {
            char::len_utf16
        } else {
            char::len_utf8
        }
    }

    /// The longest name that fits under `parent`'s path.
    fn room(&self, parent: Option<&str>) -> usize {
        let used = self.base + parent.map_or(0, |p| self.len(p) + 1);
        self.max_path.saturating_sub(used)
    }

    fn fits(&self, parent: Option<&str>, name: &str) -> bool {
        let len = self.len(name);
        len <= self.max_name && len <= self.room(parent)
    }

    /// Whether `name` fits under `parent`, and if not, the cut (and its message) that makes it.
    /// A folder cut for its path leaves room for the names inside it.
    fn fit(&self, parent: Option<&str>, name: &str, is_dir: bool) -> Fit {
        if self.fits(parent, name) {
            return Fit::Fits;
        }
        let room = self.room(parent);
        let (mut limit, key) = if self.len(name) > self.max_name && self.max_name <= room {
            (self.max_name, "names.length.name")
        } else {
            (room, "names.length.path")
        };
        if limit < MIN_NAME_LEN {
            return Fit::NoRoom(key);
        }
        if is_dir {
            limit = limit.min(room.saturating_sub(DIR_HEADROOM).max(MIN_NAME_LEN));
        }
        Fit::Cut(shorten_in(name, limit, self.width()), key)
    }

    /// `name` with the suffix ` (n)`, cut first (when `cut`) so the result still fits under
    /// `parent`.
    fn suffixed(&self, name: &str, n: u32, parent: Option<&str>, cut: bool) -> String {
        let plain = with_suffix(name, n);
        let limit = self.max_name.min(self.room(parent));
        let extra = self.len(&plain) - self.len(name);
        if !cut || self.len(&plain) <= limit || limit < MIN_NAME_LEN + extra {
            return plain;
        }
        with_suffix(&shorten_in(name, limit - extra, self.width()), n)
    }

    /// Whether `choice` on `issue` of `entry` can be honoured. Called when the user decides,
    /// so a choice that can't work is refused then and not at Send. Whether two entries end up
    /// on the same path, or a path is too long, is only known at [`NamePlan::resolve`].
    /// `issue.choice` is ignored.
    pub fn check(&self, entry: &Entry, issue: &Decided, choice: &Choice) -> Result<(), NameError> {
        let rule = issue.rule.as_str();
        match choice {
            Choice::Skip => Ok(()),
            Choice::Accept if rule == RULE_ENCODING => self.decodable(entry),
            Choice::Accept if !issue.has_proposal() => Err(NameError::NoProposal {
                path: display(entry),
            }),
            Choice::Accept => Ok(()),
            Choice::Edit(name) => self.check_edit(entry, name),
            Choice::KeepOriginal if whole_tree(rule) => Ok(()),
            Choice::KeepOriginal => self.kept_name(entry, &[rule], |_| true).map(drop),
        }
    }

    /// The final path, or skip, of every entry: the rules' result with the user's decisions
    /// applied and re-checked. `entries` in scanner order; a skipped directory skips everything
    /// under it, and a renamed one moves everything under it.
    ///
    /// Each name is worked out again from its decisions: the rules not kept out (a rule that
    /// waits for a decision applies only where it was accepted, so one added after the
    /// analysis changes nothing), then the length cut if accepted, measured under the parent's
    /// final path, then the accepted collision suffix. A name or path still too long is refused
    /// unless the user kept its length, or an ancestor's.
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
        // Whether a length was kept on the way down to each entry: past it, nothing is cut.
        let mut long_ok: Vec<bool> = Vec::with_capacity(entries.len());
        let mut seen = HashSet::new();
        for (i, entry) in entries.iter().enumerate() {
            let (parent, lenient) = match entry.parent {
                None => (None, false),
                Some(p) if p >= i => return Err(NameError::ParentAfterChild { entry: i }),
                Some(p) => match &out[p] {
                    Planned::Skipped => {
                        out.push(Planned::Skipped);
                        long_ok.push(true);
                        continue;
                    }
                    Planned::Path(pp) => (Some(pp.clone()), long_ok[p]),
                },
            };
            let decided = by_entry.get(&i).map_or(&[][..], Vec::as_slice);
            let kept_length = lenient
                || decided
                    .iter()
                    .any(|d| d.rule == RULE_LENGTH && d.choice == Some(Choice::KeepOriginal));
            long_ok.push(kept_length);
            let Some(name) = self.final_name(entry, decided, parent.as_deref(), kept_length)?
            else {
                out.push(Planned::Skipped);
                continue;
            };
            let path = join(parent.as_deref(), &name);
            if !seen.insert(self.key(&path)) {
                return Err(NameError::Duplicate { path });
            }
            out.push(Planned::Path(path));
        }
        Ok(out)
    }

    /// One entry's own final name under `parent`, or `None` to skip it. `decided` has no
    /// pending choice; `kept_length` when the user keeps this name's length (or an ancestor's).
    fn final_name(
        &self,
        entry: &Entry,
        decided: &[&Decided],
        parent: Option<&str>,
        kept_length: bool,
    ) -> Result<Option<String>, NameError> {
        let choices = || {
            decided
                .iter()
                .filter_map(|d| d.choice.as_ref().map(|c| (*d, c)))
        };
        if choices().any(|(_, c)| *c == Choice::Skip) {
            return Ok(None);
        }
        let too_long = || NameError::TooLong {
            path: display(entry),
        };
        if let Some((_, Choice::Edit(name))) =
            choices().rev().find(|(_, c)| matches!(c, Choice::Edit(_)))
        {
            self.check_edit(entry, name)?;
            if !kept_length && !self.fits(parent, name) {
                return Err(too_long());
            }
            return Ok(Some(name.clone()));
        }
        if choices().any(|(d, c)| d.rule == RULE_ENCODING && *c == Choice::Accept) {
            self.decodable(entry)?;
        }
        let kept: Vec<&str> = choices()
            .filter(|(d, c)| **c == Choice::KeepOriginal && !whole_tree(&d.rule))
            .map(|(d, _)| d.rule.as_str())
            .collect();
        let accepted = |rule: &str| {
            choices()
                .find(|(d, c)| d.rule == rule && **c == Choice::Accept)
                .map(|(d, _)| d)
        };
        let mut name = self.kept_name(entry, &kept, |issue| {
            issue.severity == Severity::Auto || accepted(issue.rule).is_some()
        })?;
        if !kept_length {
            match self.fit(parent, &name, entry.is_dir) {
                Fit::Fits => {}
                Fit::Cut(short, _) if accepted(RULE_LENGTH).is_some() => name = short,
                Fit::Cut(..) | Fit::NoRoom(_) => return Err(too_long()),
            }
        }
        if let Some(d) = accepted(RULE_COLLISION) {
            let n = suffix_number(&String::from_utf8_lossy(&d.after)).unwrap_or(2);
            name = self.suffixed(&name, n, parent, !kept_length);
        }
        valid(&name)?;
        Ok(Some(name))
    }

    /// A name the user typed: well formed, no longer than the destination takes, and left
    /// alone by every active rule.
    fn check_edit(&self, entry: &Entry, name: &str) -> Result<(), NameError> {
        valid(name)?;
        if self.len(name) > self.max_name {
            return Err(NameError::TooLong {
                path: display(entry),
            });
        }
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

    /// The entry's name with the rules in `kept` left out, applying only the issues `apply`
    /// lets through. Refused when that name is not valid UTF-8, or when leaving rules out makes
    /// another rule change it instead (say Nextcloud's own NFC rule, once the general one is
    /// kept out).
    fn kept_name(
        &self,
        entry: &Entry,
        kept: &[&str],
        apply: impl Fn(&Issue) -> bool,
    ) -> Result<String, NameError> {
        if kept.contains(&RULE_ENCODING) {
            return Err(NameError::Undecodable {
                path: display(entry),
            });
        }
        let (name, issues) = self.rules.run_where(entry, kept, apply);
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
    /// one), and, on a destination that ignores case, their case-folded form.
    fn key(&self, path: &str) -> String {
        if self.case_sensitive {
            path.nfc().collect()
        } else {
            loose_key(path)
        }
    }
}

/// `length` and `collision` look at the whole tree; keeping their original is always possible.
fn whole_tree(rule: &str) -> bool {
    rule == RULE_COLLISION || rule == RULE_LENGTH
}

fn join(parent: Option<&str>, name: &str) -> String {
    parent.map_or(name.to_owned(), |p| format!("{p}/{name}"))
}

/// The form two names share when a destination that normalises Unicode and ignores case would
/// store them as one: NFC, then full case folding (`ß` and `SS`, `ς` and `σ`, `ﬁ` and `fi`
/// fold together).
pub fn loose_key(name: &str) -> String {
    let nfc: String = name.nfc().collect();
    nfc.to_uppercase().to_lowercase().nfc().collect()
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
    shorten_in(name, max, char::len_utf8)
}

/// [`shorten`], with each character `width` long (UTF-8 bytes, UTF-16 units).
fn shorten_in(name: &str, max: usize, width: fn(char) -> usize) -> String {
    let len = |s: &str| s.chars().map(width).sum::<usize>();
    if len(name) <= max {
        return name.to_owned();
    }
    // Byte index where the longest start of `s` at most `n` long ends.
    let head_of = |s: &str, n: usize| {
        let mut used = 0;
        for (i, c) in s.char_indices() {
            used += width(c);
            if used > n {
                return i;
            }
        }
        s.len()
    };
    // Byte index where the longest end of `s` at most `n` long starts.
    let tail_of = |s: &str, n: usize| {
        let mut used = 0;
        for (i, c) in s.char_indices().rev() {
            used += width(c);
            if used > n {
                return i + c.len_utf8();
            }
        }
        0
    };
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 && name.len() - dot <= 16 => name.split_at(dot),
        _ => (name, ""),
    };
    let Some(room) = max.checked_sub(len(ext) + len(CUT)).filter(|r| *r >= 2) else {
        return name[..head_of(name, max)].to_owned();
    };
    let head = head_of(stem, room / 2);
    let rest = &stem[head..];
    let tail = head + tail_of(rest, room - len(&stem[..head]));
    format!("{}{CUT}{}{ext}", &stem[..head], &stem[tail..])
}

/// `photo.jpg` → `photo (2).jpg`; `.bashrc` and `README` get the suffix at the end.
pub fn with_suffix(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

/// The `n` of a name [`with_suffix`] made: `photo (3).jpg` → 3.
pub fn suffix_number(name: &str) -> Option<u32> {
    let number =
        |s: &str| -> Option<u32> { s.strip_suffix(')')?.rsplit_once(" (")?.1.parse().ok() };
    let stem = match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    };
    number(stem).or_else(|| number(name))
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
                    before: issue.before.clone(),
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

    /// The `rule` issue of entry `i`, as the analysis proposes it.
    fn issue(plan: &NamePlan, entries: &[Entry], i: EntryId, rule: &str) -> Decided {
        defaults(&plan.propose(entries))
            .into_iter()
            .find(|d| d.entry == i && d.rule == rule)
            .expect("issue exists")
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

        let encoding = issue(&plan, &entries, 0, RULE_ENCODING);
        for choice in [Choice::Accept, Choice::KeepOriginal] {
            assert!(plan.check(&entries[0], &encoding, &choice).is_err());
        }
        assert_eq!(plan.check(&entries[0], &encoding, &Choice::Skip), Ok(()));
        let edit = Choice::Edit("Patológica.pdf".into());
        assert_eq!(plan.check(&entries[0], &encoding, &edit), Ok(()));
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
            .check(
                &entries[0],
                &issue(&plan, &entries, 0, "nfc"),
                &Choice::KeepOriginal,
            )
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
        let forbidden = issue(&plan, &entries, 0, "forbidden-chars");
        let check = |name: &str| plan.check(&entries[0], &forbidden, &Choice::Edit(name.into()));
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
            plan.check(
                &entries[0],
                &issue(&plan, &entries, 0, RULE_ENCODING),
                &Choice::Accept
            ),
            Ok(())
        );
        let decisions = defaults(&mapped);
        assert_eq!(plan.resolve(&entries, &decisions).unwrap(), [path("Acção")]);
    }

    fn limited_on(target: TargetProfile) -> NamePlan {
        NamePlan::for_target(
            RuleSet::new(general_rules(ActiveFor::windows()), &target),
            &target,
        )
    }

    fn general(max_name: usize, max_path: usize) -> NamePlan {
        limited_on(TargetProfile {
            max_name_bytes: max_name,
            max_path_bytes: max_path,
            ..TargetProfile::default()
        })
    }

    #[test]
    fn suffix_numbers_are_read_back() {
        assert_eq!(suffix_number("photo (3).jpg"), Some(3));
        assert_eq!(suffix_number(".bashrc (2)"), Some(2));
        assert_eq!(suffix_number("README (12)"), Some(12));
        assert_eq!(suffix_number("photo.jpg"), None);
    }

    #[test]
    fn a_kept_rule_stays_kept_under_an_accepted_cut_or_suffix() {
        // The cut and the suffix are worked out again from the kept name: the garbled spelling
        // the user kept is not repaired behind their back.
        let entries = tree(&[(None, "PatolÃ³gica de 2023 em revisao.txt".as_bytes())]);
        let plan = general(24, 4096);
        let mut decisions = defaults(&plan.propose(&entries));
        set(&mut decisions, 0, "mojibake", Choice::KeepOriginal);
        set(&mut decisions, 0, RULE_LENGTH, Choice::Accept);
        let Planned::Path(name) = &plan.resolve(&entries, &decisions).unwrap()[0] else {
            panic!("skipped")
        };
        assert!(name.starts_with("PatolÃ"), "{name}");
        assert!(name.len() <= 24, "{name}");

        let entries = tree(&[
            (None, "Patológica.txt".as_bytes()),
            (None, "PatolÃ³gica.txt".as_bytes()),
        ]);
        let plan = general(255, 4096);
        let mapped = plan.propose(&entries);
        assert_eq!(mapped[1].issues[1].rule, RULE_COLLISION);
        let mut decisions = defaults(&mapped);
        set(&mut decisions, 1, "mojibake", Choice::KeepOriginal);
        set(&mut decisions, 1, RULE_COLLISION, Choice::Accept);
        assert_eq!(
            plan.resolve(&entries, &decisions).unwrap()[1],
            path("PatolÃ³gica (2).txt")
        );
    }

    #[test]
    fn a_cut_folder_leaves_room_and_its_children_are_measured_under_the_cut() {
        let folder = "d".repeat(80);
        let child = format!("{folder}/{}", "f".repeat(30));
        let mut entries = tree(&[(None, folder.as_bytes()), (Some(0), child.as_bytes())]);
        entries[0].is_dir = true;
        let plan = limited(255, 60);
        let mapped = plan.propose(&entries);
        assert_eq!(mapped[0].issues[0].rule, RULE_LENGTH);
        assert!(
            mapped[1].issues.is_empty(),
            "the child fits under the cut folder: {:?}",
            mapped[1].issues
        );
        let mut decisions = defaults(&mapped);
        set(&mut decisions, 0, RULE_LENGTH, Choice::Accept);
        let planned = plan.resolve(&entries, &decisions).unwrap();
        let Planned::Path(p) = &planned[1] else {
            panic!("skipped")
        };
        assert!(p.len() <= 60 && p.ends_with(&"f".repeat(30)), "{p}");
    }

    #[test]
    fn no_cut_fits_is_an_issue_without_a_proposal() {
        let folder = "d".repeat(30);
        let child = format!("{folder}/{}", "f".repeat(20));
        let entries = tree(&[(None, folder.as_bytes()), (Some(0), child.as_bytes())]);
        let plan = limited(255, 40);
        let mapped = plan.propose(&entries);
        let length = issue(&plan, &entries, 1, RULE_LENGTH);
        assert!(!length.has_proposal(), "{mapped:?}");
        assert!(matches!(
            plan.check(&entries[1], &length, &Choice::Accept),
            Err(NameError::NoProposal { .. })
        ));
        let mut decisions = defaults(&mapped);
        set(&mut decisions, 1, RULE_LENGTH, Choice::Edit("x".into()));
        assert_eq!(
            plan.resolve(&entries, &decisions).unwrap()[1],
            path(&format!("{folder}/x"))
        );
        set(&mut decisions, 1, RULE_LENGTH, Choice::KeepOriginal);
        assert_eq!(
            plan.resolve(&entries, &decisions).unwrap()[1],
            path(&child),
            "kept: sent as it is"
        );
    }

    #[test]
    fn length_is_enforced_when_deciding_and_at_send() {
        let child = format!("d/{}", "f".repeat(30));
        let entries = tree(&[(None, b"d"), (Some(0), child.as_bytes())]);
        let plan = limited(30, 40);
        let mapped = plan.propose(&entries);
        assert!(mapped[0].issues.is_empty());
        // An edit longer than a name may be is refused when decided.
        let mut decisions = defaults(&mapped);
        decisions.push(Decided {
            entry: 0,
            rule: "nfc".into(),
            before: b"d".to_vec(),
            after: b"d".to_vec(),
            choice: Some(Choice::Edit("e".repeat(31))),
        });
        assert!(matches!(
            plan.check(
                &entries[0],
                &decisions[decisions.len() - 1],
                &Choice::Edit("e".repeat(31))
            ),
            Err(NameError::TooLong { .. })
        ));
        // A folder edited longer pushes its child past the path limit: refused at Send.
        let last = decisions.len() - 1;
        decisions[last].choice = Some(Choice::Edit("e".repeat(20)));
        assert!(matches!(
            plan.resolve(&entries, &decisions),
            Err(NameError::TooLong { .. })
        ));
    }

    #[test]
    fn a_collision_suffix_fits_the_limit() {
        let target = TargetProfile {
            max_name_bytes: 20,
            case_sensitive: false,
            ..TargetProfile::default()
        };
        let plan = NamePlan::for_target(RuleSet::new(vec![Box::new(Nfc)], &target), &target);
        let entries = tree(&[
            (None, "a".repeat(20).as_bytes()),
            (None, "A".repeat(20).as_bytes()),
        ]);
        let mapped = plan.propose(&entries);
        let after = String::from_utf8(mapped[1].issues[0].after.clone()).unwrap();
        assert_eq!(mapped[1].issues[0].rule, RULE_COLLISION);
        assert!(after.len() <= 20 && after.ends_with(" (2)"), "{after}");
        let mut decisions = defaults(&mapped);
        set(&mut decisions, 1, RULE_COLLISION, Choice::Accept);
        assert_eq!(plan.resolve(&entries, &decisions).unwrap()[1], path(&after));
    }

    #[test]
    fn the_target_folder_counts_against_the_path_limit() {
        let entries = tree(&[(None, "f".repeat(40).as_bytes())]);
        assert!(limited(255, 50).propose(&entries)[0].issues.is_empty());
        let plan = limited(255, 50).under(Some(&"x".repeat(20)));
        let mapped = plan.propose(&entries);
        assert_eq!(mapped[0].issues[0].rule, RULE_LENGTH);
        assert!(20 + 1 + mapped[0].issues[0].after.len() <= 50);
    }

    #[test]
    fn windows_lengths_are_utf16_units() {
        // 20 accented letters: 40 UTF-8 bytes, 20 UTF-16 units.
        let entries = tree(&[(None, "é".repeat(20).as_bytes())]);
        let on = |family| {
            let target = TargetProfile {
                family,
                max_name_bytes: 20,
                ..TargetProfile::default()
            };
            NamePlan::for_target(RuleSet::new(vec![Box::new(Nfc)], &target), &target)
                .propose(&entries)[0]
                .issues
                .len()
        };
        assert_eq!(on(TargetFamily::Windows), 0);
        assert_eq!(on(TargetFamily::Unix), 1);
    }

    #[test]
    fn a_rule_that_waits_for_a_decision_needs_one_to_apply() {
        // As if `mojibake` came after the analysis: it has no decision, so it changes nothing.
        let entries = tree(&[(None, "PatolÃ³gica.txt".as_bytes())]);
        let plan = general(255, 4096);
        assert_eq!(
            plan.resolve(&entries, &[]).unwrap(),
            [path("PatolÃ³gica.txt")]
        );
    }

    #[test]
    fn case_folding_is_full() {
        assert_eq!(loose_key("Straße"), loose_key("STRASSE"));
        assert_eq!(loose_key("ﬁle"), loose_key("FILE"));
        assert_eq!(loose_key("Vera\u{303}o"), loose_key("VERÃO"));
        let entries = tree(&[(None, "Straße.txt".as_bytes()), (None, b"STRASSE.txt")]);
        let collisions = nfc_only(false)
            .propose(&entries)
            .iter()
            .flat_map(|m| &m.issues)
            .filter(|i| i.rule == RULE_COLLISION)
            .count();
        assert_eq!(collisions, 1);
    }
}
