//! `NameRule` trait and the general name-correction rules.
//!
//! Pure and synchronous: no I/O. Names arrive as raw bytes (the scanner never assumes UTF-8) and
//! leave as a `source → destination` path map. Per-name rules: `encoding`, `mojibake`, `nfc`,
//! `forbidden-chars`, `trailing` and `reserved`; the ones that need the whole tree, `length` and
//! `collision`, are part of the [`NamePlan`] (see `docs/04-name-correction.md`).

mod encoding;
mod forbidden_chars;
mod mojibake;
mod nfc;
mod plan;
mod reserved;
mod rule_set;
mod trailing;

pub use encoding::Encoding;
pub use forbidden_chars::ForbiddenChars;
pub use mojibake::Mojibake;
pub use nfc::Nfc;
pub use plan::{
    Choice, Decided, NameError, NamePlan, Planned, RULE_COLLISION, RULE_ENCODING, RULE_LENGTH,
    shorten, with_suffix,
};
pub use reserved::{Reserved, WINDOWS_DEVICE_NAMES};
pub use rule_set::{Mapped, RULE_ORDER, RuleSet};
pub use trailing::Trailing;

/// Index of an [`Entry`] inside the slice produced by the scanner.
pub type EntryId = usize;

/// One source file or directory, as seen by the scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Path relative to the source root, as raw bytes (`/`-separated on Unix).
    pub raw_path: Vec<u8>,
    /// Last path component, as raw bytes.
    pub raw_name: Vec<u8>,
    /// `raw_name` decoded, if it is valid UTF-8.
    pub name: Option<String>,
    pub size: u64,
    pub is_dir: bool,
    /// Parent directory, `None` for direct children of the source root.
    pub parent: Option<EntryId>,
}

impl Entry {
    /// Builds an entry from a raw name, decoding it if it is valid UTF-8.
    pub fn new(
        raw_path: Vec<u8>,
        raw_name: Vec<u8>,
        size: u64,
        is_dir: bool,
        parent: Option<EntryId>,
    ) -> Self {
        let name = String::from_utf8(raw_name.clone()).ok();
        Self {
            raw_path,
            raw_name,
            name,
            size,
            is_dir,
            parent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Applied by default; the user can override.
    Auto,
    /// Blocks Send until the user approves, edits or skips.
    NeedsDecision,
}

/// Something a rule found wrong with a name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub rule: &'static str,
    pub severity: Severity,
    /// i18n key for the human message.
    pub message_key: &'static str,
    /// Name before and after, as bytes, for display (NFC changes are invisible as text).
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFamily {
    Unix,
    Windows,
    MacOs,
}

/// What the destination accepts; decides which general rules apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetProfile {
    pub family: TargetFamily,
    pub max_name_bytes: usize,
    pub max_path_bytes: usize,
    pub case_sensitive: bool,
    pub integration: Option<String>,
}

impl Default for TargetProfile {
    fn default() -> Self {
        Self {
            family: TargetFamily::Unix,
            max_name_bytes: 255,
            max_path_bytes: 4096,
            case_sensitive: true,
            integration: None,
        }
    }
}

/// Which targets a target-dependent rule is active for: Windows targets, targets whose
/// integration is in a list, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveFor {
    windows: bool,
    integrations: Vec<String>,
}

impl ActiveFor {
    /// Windows targets only.
    pub fn windows() -> Self {
        Self {
            windows: true,
            integrations: Vec::new(),
        }
    }

    /// Only targets whose integration id is one of `ids`, whatever their family.
    pub fn integrations<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            windows: false,
            integrations: ids.into_iter().map(Into::into).collect(),
        }
    }

    /// Also targets whose integration id is one of `ids`.
    pub fn and_integrations<I, S>(mut self, ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.integrations.extend(ids.into_iter().map(Into::into));
        self
    }

    pub fn matches(&self, target: &TargetProfile) -> bool {
        (self.windows && target.family == TargetFamily::Windows)
            || target
                .integration
                .as_deref()
                .is_some_and(|id| self.integrations.iter().any(|i| i == id))
    }
}

/// The general per-name rules, wired as `docs/04` describes: `encoding`, `mojibake` and `nfc` always;
/// `forbidden-chars` and `trailing` on the targets in `windows_compat` (Windows, plus any
/// integration that requires them); `reserved` with the Windows device names on Windows targets.
///
/// `RuleSet::new` filters them by target and puts them in order.
pub fn general_rules(windows_compat: ActiveFor) -> Vec<Box<dyn NameRule>> {
    vec![
        Box::new(Encoding),
        Box::new(Mojibake),
        Box::new(Nfc),
        Box::new(ForbiddenChars::new(windows_compat.clone())),
        Box::new(Trailing::new(windows_compat)),
        Box::new(Reserved::windows()),
    ]
}

pub trait NameRule: Send + Sync {
    fn id(&self) -> &str;
    fn applies_to(&self, target: &TargetProfile) -> bool;
    /// What is wrong with `entry`'s name, if anything; the issue's `after` is the corrected name.
    fn check(&self, entry: &Entry) -> Option<Issue>;
}
