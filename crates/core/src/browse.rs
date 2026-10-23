//! Folders at a destination, to pick a profile's root or the folder a transfer goes into, and the
//! check of a folder name typed there (docs/design/specs/D11). Listing only reads; nothing is
//! created here: a new folder is made by the send itself, like every other parent folder.

use std::cmp::Ordering;

use filebridge_model::{BrowseResult, BrowseTarget, FolderNameCheck, RemoteEntryView};
use filebridge_names::Entry;
use filebridge_transport_api::{RemotePath, TransportCaps};

use crate::{CoreError, Engine, Profile, Result};

/// The most folders one listing returns; narrow with `prefix` for the rest.
pub const MAX_FOLDERS: usize = 2000;

impl Engine {
    async fn browse_profile(&self, target: BrowseTarget) -> Result<Profile> {
        match target {
            BrowseTarget::Profile { id } => self.get_profile(&id).await,
            BrowseTarget::Draft { profile } => {
                self.registry.check_profile(&profile)?;
                Ok(*profile)
            }
        }
    }

    /// The folders directly inside `dir` (the destination root when `None`), naturally sorted,
    /// at most [`MAX_FOLDERS`]; with `prefix`, only those whose name starts with it.
    pub async fn browse(
        &self,
        target: BrowseTarget,
        dir: Option<String>,
        prefix: Option<String>,
    ) -> Result<BrowseResult> {
        let profile = self.browse_profile(target).await?;
        let transport = self.registry.transport(&profile.destination.transport)?;
        if !transport.capabilities().contains(TransportCaps::LIST) {
            return Err(CoreError::Invalid(
                "this destination can't list its folders".into(),
            ));
        }
        let dir = dir
            .map(|d| d.trim_matches('/').to_owned())
            .filter(|d| !d.is_empty());
        let remote = dir.as_deref().map(RemotePath::new).transpose()?;
        let mut folders = transport
            .list(&profile.destination, remote.as_ref(), true)
            .await?;
        if let Some(p) = prefix
            .as_deref()
            .map(str::to_lowercase)
            .filter(|p| !p.is_empty())
        {
            folders.retain(|f| f.name.to_lowercase().starts_with(&p));
        }
        folders.sort_by(|a, b| natural(&a.name, &b.name));
        let truncated = folders.len() > MAX_FOLDERS;
        folders.truncate(MAX_FOLDERS);
        let path = dir.unwrap_or_default();
        Ok(BrowseResult {
            segments: path
                .split('/')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            folders: folders
                .into_iter()
                .map(|f| RemoteEntryView {
                    path: if path.is_empty() {
                        f.name.clone()
                    } else {
                        format!("{path}/{}", f.name)
                    },
                    name: f.name,
                    mtime: f.mtime,
                })
                .collect(),
            path,
            truncated,
        })
    }

    /// Whether `name` is acceptable as a folder name at the destination, and the fix if not.
    pub async fn check_folder_name(
        &self,
        target: BrowseTarget,
        name: String,
    ) -> Result<FolderNameCheck> {
        let profile = self.browse_profile(target).await?;
        Ok(check_name(&self.registry.name_plan(&profile)?, &name))
    }

    /// The folder the last transfer to this profile was sent into, for the wizard to preselect:
    /// `None` for its root, before its first send, or when the profile's name rules no longer
    /// accept that folder (it was edited since).
    pub async fn last_target_dir(&self, profile_id: &str) -> Result<Option<String>> {
        let profile = self.get_profile(profile_id).await?;
        let dir = self.store.last_target_dir(profile_id).await?;
        Ok(dir.filter(|d| self.check_target_dir(&profile, Some(d)).is_ok()))
    }

    /// A transfer's target folder must be a relative path whose every folder name the
    /// destination accepts as it is.
    pub(crate) fn check_target_dir(&self, profile: &Profile, dir: Option<&str>) -> Result<()> {
        let Some(dir) = dir else { return Ok(()) };
        RemotePath::new(dir)?;
        let plan = self.registry.name_plan(profile)?;
        for part in dir.split('/') {
            let c = check_name(&plan, part);
            if !c.ok {
                return Err(CoreError::Invalid(format!(
                    "the destination can't store the folder name {part:?}{}",
                    c.proposed
                        .map(|p| format!("; use {p:?}"))
                        .unwrap_or_default()
                )));
            }
        }
        Ok(())
    }
}

fn check_name(plan: &filebridge_names::NamePlan, name: &str) -> FolderNameCheck {
    let raw = name.as_bytes().to_vec();
    let entry = Entry::new(raw.clone(), raw, 0, true, None);
    let m = plan.propose(std::slice::from_ref(&entry)).remove(0);
    let ok = m.issues.is_empty() && !name.is_empty() && !name.contains('/');
    FolderNameCheck {
        ok,
        proposed: (!ok)
            .then(|| m.dest_name.clone())
            .flatten()
            .filter(|p| p != name),
        message_key: m.issues.first().map(|i| i.message_key.to_owned()),
    }
}

/// Puts a transfer's target folder in front of a destination path.
pub(crate) fn under(target: Option<&str>, path: String) -> String {
    match target
        .map(|t| t.trim_matches('/'))
        .filter(|t| !t.is_empty())
    {
        Some(t) => format!("{t}/{path}"),
        None => path,
    }
}

/// "2" before "10": compares runs of digits by value, the rest case-insensitively.
fn natural(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let n = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut s = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        s.push(c);
                        it.next();
                    }
                    s
                };
                let (p, q) = (n(&mut x), n(&mut y));
                let (p, q) = (p.trim_start_matches('0'), q.trim_start_matches('0'));
                match p.len().cmp(&q.len()).then_with(|| p.cmp(q)) {
                    Ordering::Equal => {}
                    o => return o,
                }
            }
            (Some(c), Some(d)) => {
                match c.to_lowercase().cmp(d.to_lowercase()) {
                    Ordering::Equal => {}
                    o => return o,
                }
                x.next();
                y.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut v = vec!["10", "2", "Fotos", "fotos 3", "Fotos 12", "a"];
        v.sort_by(|a, b| natural(a, b));
        assert_eq!(v, ["2", "10", "a", "Fotos", "fotos 3", "Fotos 12"]);
    }

    #[test]
    fn target_folders_go_in_front() {
        assert_eq!(
            under(Some("2019/Novas/"), "Acme/a.jpg".into()),
            "2019/Novas/Acme/a.jpg"
        );
        assert_eq!(under(None, "a".into()), "a");
        assert_eq!(under(Some(""), "a".into()), "a");
    }
}
