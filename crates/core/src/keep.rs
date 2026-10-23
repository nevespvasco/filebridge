//! Keep the older version (`SendOptions::keep_older`, docs/adr/0019): before a batch is sent, a
//! file whose destination name is already taken by other contents is renamed in its folder to
//! `<name> (filebridge <YYYY-MM-DD HHMM>)<ext>`, so the new version never overwrites it.
//!
//! Each move goes through `Transport::move_path` with an intermediate name derived from the job
//! and the entry, so a move cut short is finished when the batch runs again. Nothing at the
//! destination is ever deleted or overwritten with different contents.

use std::collections::HashMap;
use std::path::PathBuf;

use filebridge_names::{loose_key, with_suffix};
use filebridge_store::{EntryState, ManifestEntry};
use filebridge_transport_api::{
    Destination, ErrorKind, HashAlgo, MoveRequest, RemoteEntry, RemotePath, Transport,
    TransportCaps,
};
use tokio_util::sync::CancellationToken;

use crate::analyse::{bytes_path, source_parent};
use crate::{CoreError, Engine, Result};

/// How the step before a batch ended.
pub(crate) enum Kept {
    /// These entries go: their names are free, or hold the same file.
    Go(Vec<ManifestEntry>),
    /// The destination didn't answer; the stage backs off and runs again.
    Retry(String),
    Cancelled,
}

/// What one batch's step needs from the job.
pub(crate) struct KeepCtx<'a> {
    pub job_id: &'a str,
    pub transport: &'a dyn Transport,
    pub dest: &'a Destination,
    pub sources: &'a [PathBuf],
    /// When the run started: the date in the kept names.
    pub started_at: i64,
    pub cancel: &'a CancellationToken,
}

/// The intermediate name of an entry's move, in its folder.
fn intermediate(job_id: &str, idx: u64) -> String {
    let job: String = job_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect();
    format!(".fb-keep-{job}-{idx}")
}

/// `photo.jpg` → `photo (filebridge 2026-10-23 0300).jpg`, with ` (2)`, ` (3)`… while that
/// is taken in the folder (exactly, in another normalisation or in another case).
pub(crate) fn kept_name(name: &str, stamp: &str, taken: &[RemoteEntry]) -> String {
    let base = match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} (filebridge {stamp}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} (filebridge {stamp})"),
    };
    let used = |n: &str| {
        let key = loose_key(n);
        taken.iter().any(|e| loose_key(&e.name) == key)
    };
    let mut candidate = base.clone();
    let mut n = 2;
    while used(&candidate) {
        candidate = with_suffix(&base, n);
        n += 1;
    }
    candidate
}

fn join(folder: Option<&str>, name: &str) -> String {
    match folder {
        Some(f) => format!("{f}/{name}"),
        None => name.to_owned(),
    }
}

impl Engine {
    /// Frees the destination names of `entries` that other contents hold, keeping those
    /// contents under a dated name. Entries that can't go are saved `Failed` with the reason.
    pub(crate) async fn keep_older(
        &self,
        ctx: &KeepCtx<'_>,
        entries: Vec<ManifestEntry>,
    ) -> Result<Kept> {
        let can_move = ctx.transport.capabilities().contains(TransportCaps::MOVE);
        let stamp = self.options.wall_clock.stamp(ctx.started_at);
        let mut listings: HashMap<Option<String>, Vec<RemoteEntry>> = HashMap::new();
        let (mut go, mut failed, mut kept) = (Vec::new(), Vec::new(), 0u64);
        for e in entries {
            if ctx.cancel.is_cancelled() {
                return Ok(Kept::Cancelled);
            }
            // No destination name: the batch fails it with the reason.
            let Some(path) = e.dest_path.clone() else {
                go.push(e);
                continue;
            };
            let (folder, name) = match path.rsplit_once('/') {
                Some((f, n)) => (Some(f.to_owned()), n.to_owned()),
                None => (None, path.clone()),
            };
            if !listings.contains_key(&folder) {
                let dir = folder.clone().map(RemotePath::new).transpose()?;
                let listing = match ctx.transport.list(ctx.dest, dir.as_ref(), false).await {
                    Ok(l) => l,
                    Err(err) if err.kind == ErrorKind::NotFound => Vec::new(),
                    Err(err) if err.is_retryable() => return Ok(Kept::Retry(err.message)),
                    Err(err) => {
                        failed.push((e.entry.idx, err.message));
                        continue;
                    }
                };
                listings.insert(folder.clone(), listing);
            }
            let listing = listings.get_mut(&folder).expect("listed above");
            let via = intermediate(ctx.job_id, e.entry.idx);
            let taken = listing.iter().find(|x| x.name == name).cloned();
            // A move cut short leaves the older version under `via`: finish it.
            let half_done = listing.iter().find(|x| x.name == via).cloned();
            let older = match (taken, half_done) {
                (None, None) => {
                    go.push(e);
                    continue;
                }
                (Some(x), _) if x.is_dir => {
                    failed.push((
                        e.entry.idx,
                        format!("a folder named {name} is in the way at the destination"),
                    ));
                    continue;
                }
                (Some(x), None) => {
                    match self.same_file(ctx, &e, &path, &x).await? {
                        Same::Yes => {
                            go.push(e);
                            continue;
                        }
                        Same::Retry(msg) => return Ok(Kept::Retry(msg)),
                        Same::No => {}
                    }
                    x
                }
                (_, Some(x)) => x,
            };
            if !can_move {
                failed.push((
                    e.entry.idx,
                    "an older version is at the destination, and this destination can't rename \
                     it to keep it"
                        .into(),
                ));
                continue;
            }
            let to_name = kept_name(&name, &stamp, listing);
            let req = MoveRequest {
                from: RemotePath::new(path.clone())?,
                to: RemotePath::new(join(folder.as_deref(), &to_name))?,
                via: RemotePath::new(join(folder.as_deref(), &via))?,
                is_dir: false,
                size: older.size,
                mtime: older.mtime,
            };
            match ctx.transport.move_path(ctx.dest, &req).await {
                Ok(()) => {
                    listing.retain(|x| x.name != name && x.name != via);
                    listing.push(RemoteEntry {
                        name: to_name,
                        ..older
                    });
                    self.store
                        .set_kept_as(ctx.job_id, e.entry.idx, req.to.to_string())
                        .await?;
                    kept += 1;
                    go.push(e);
                }
                Err(err) if err.is_retryable() => return Ok(Kept::Retry(err.message)),
                Err(err) => failed.push((
                    e.entry.idx,
                    format!("the older version could not be kept: {}", err.message),
                )),
            }
        }
        if kept > 0 {
            self.log(
                Some(ctx.job_id),
                "kept_older",
                serde_json::json!({ "count": kept }),
            )
            .await?;
        }
        if !failed.is_empty() {
            self.store
                .set_transfer_states(
                    ctx.job_id,
                    failed
                        .into_iter()
                        .map(|(idx, msg)| (idx, EntryState::Failed, Some(msg)))
                        .collect(),
                )
                .await?;
        }
        Ok(Kept::Go(go))
    }

    /// Whether the file at the destination is the entry's own contents: same size, and the same
    /// SHA-256 on both sides. A destination that can't hash says no (the older one is kept).
    async fn same_file(
        &self,
        ctx: &KeepCtx<'_>,
        e: &ManifestEntry,
        path: &str,
        there: &RemoteEntry,
    ) -> Result<Same> {
        if there.size != Some(e.entry.size)
            || !ctx
                .transport
                .capabilities()
                .contains(TransportCaps::HASH_SHA256)
        {
            return Ok(Same::No);
        }
        let local = ctx
            .sources
            .get(e.entry.source as usize)
            .map(|s| source_parent(s).join(bytes_path(&e.entry.raw_path)))
            .ok_or_else(|| CoreError::Invalid("unknown source".into()))?;
        let local = tokio::task::spawn_blocking(move || crate::run::sha256_file(&local))
            .await
            .map_err(|err| CoreError::Invalid(format!("hash task failed: {err}")))?;
        // The source can't be read: the batch reports it when it tries to send it.
        let Ok(local) = local else {
            return Ok(Same::Yes);
        };
        let remote = ctx
            .transport
            .hash_many(
                ctx.dest,
                &[RemotePath::new(path.to_owned())?],
                HashAlgo::Sha256,
            )
            .await
            .pop();
        Ok(match remote {
            Some(Ok(d)) if d.bytes == local => Same::Yes,
            Some(Err(err)) if err.is_retryable() => Same::Retry(err.message),
            _ => Same::No,
        })
    }
}

enum Same {
    Yes,
    No,
    Retry(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str) -> RemoteEntry {
        RemoteEntry {
            name: name.into(),
            is_dir: false,
            size: Some(1),
            mtime: None,
        }
    }

    #[test]
    fn kept_names_are_dated_and_free() {
        let stamp = "2026-10-23 0300";
        assert_eq!(
            kept_name("photo.jpg", stamp, &[]),
            "photo (filebridge 2026-10-23 0300).jpg"
        );
        assert_eq!(
            kept_name("README", stamp, &[]),
            "README (filebridge 2026-10-23 0300)"
        );
        assert_eq!(
            kept_name(".bashrc", stamp, &[]),
            ".bashrc (filebridge 2026-10-23 0300)"
        );
        let taken = [
            file("PHOTO (filebridge 2026-10-23 0300).jpg"),
            file("photo (filebridge 2026-10-23 0300) (2).jpg"),
        ];
        assert_eq!(
            kept_name("photo.jpg", stamp, &taken),
            "photo (filebridge 2026-10-23 0300) (3).jpg"
        );
    }

    #[test]
    fn intermediate_names_are_hidden_and_per_entry() {
        assert_eq!(
            intermediate("a1b2-c3d4-e5f6-0000", 7),
            ".fb-keep-a1b2c3d4e5f6-7"
        );
    }
}
