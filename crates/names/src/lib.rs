//! `NameRule` trait and the general name-correction rules.
//!
//! Pure and synchronous: no I/O. Names arrive as raw bytes (the scanner never assumes UTF-8) and
//! leave as a `source → destination` path map. Phase 0 ships only the `nfc` rule; the other
//! general rules (see `docs/04-name-correction.md`) land in later phases.

mod nfc;
mod rule_set;

pub use nfc::Nfc;
pub use rule_set::{Mapped, RuleSet};

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

pub trait NameRule: Send + Sync {
    fn id(&self) -> &str;
    fn applies_to(&self, target: &TargetProfile) -> bool;
    /// What is wrong with `entry`'s name, if anything.
    fn check(&self, entry: &Entry) -> Option<Issue>;
    /// The corrected name, if this rule would change it.
    fn propose(&self, entry: &Entry) -> Option<String>;
}
