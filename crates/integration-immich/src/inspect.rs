//! What the profile wizard and every preflight check: Immich answers, the key is an
//! administrator's, the library is there and imports the destination's folder, and Immich can
//! read that folder. Each step is a [`Check`]; nothing is changed on the server.

use std::time::Instant;

use filebridge_integration_api::{Check, CheckStatus};

use crate::api::{Api, ApiErrorKind, Library};
use crate::settings::covers;

/// The outcome: the checks in order, and the library when one was found.
#[derive(Debug, Clone, Default)]
pub struct Inspection {
    pub checks: Vec<Check>,
    pub library: Option<Library>,
    /// The library's import path that holds the folder.
    pub import_root: Option<String>,
    /// Libraries to choose from, when none was named and the folder didn't pick exactly one.
    pub candidates: Vec<Library>,
    /// How long the ping took.
    pub latency_ms: Option<u64>,
}

impl Inspection {
    fn push(&mut self, id: &str, status: CheckStatus, message: impl Into<String>) {
        self.checks.push(Check {
            id: id.into(),
            status,
            message: message.into(),
        });
    }

    /// A library was found, imports the folder, and nothing failed.
    pub fn passed(&self) -> bool {
        self.library.is_some()
            && self.import_root.is_some()
            && !self.checks.iter().any(|c| c.status == CheckStatus::Fail)
    }
}

fn describe(libraries: &[Library]) -> String {
    libraries
        .iter()
        .map(|l| format!("{} ({}: {})", l.name, l.id, l.import_paths.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Runs the checks against `server` with `key`. `library_id` `None` picks the one library that
/// imports `import_path` (the folder as Immich sees it).
pub async fn inspect(
    api: &Api,
    server: &str,
    key: &str,
    library_id: Option<&str>,
    import_path: &str,
) -> Inspection {
    let mut out = Inspection::default();

    let started = Instant::now();
    if let Err(e) = api.ping(server).await {
        out.push("server", CheckStatus::Fail, e.message);
        return out;
    }
    out.latency_ms = Some(started.elapsed().as_millis() as u64);
    let version = api.version(server).await.ok();
    out.push(
        "server",
        CheckStatus::Ok,
        match version {
            Some(v) => format!("Immich {v} answers at {server}"),
            None => format!("Immich answers at {server}"),
        },
    );

    let found = match library_id {
        Some(id) => api.library(server, key, id).await.map(Some),
        None => api.libraries(server, key).await.map(|all| {
            let holding: Vec<Library> = all
                .iter()
                .filter(|l| l.import_paths.iter().any(|p| covers(p, import_path)))
                .cloned()
                .collect();
            match holding.len() {
                1 => holding.into_iter().next(),
                0 => {
                    out.candidates = all;
                    None
                }
                _ => {
                    out.candidates = holding;
                    None
                }
            }
        }),
    };
    let library = match found {
        Ok(found) => {
            out.push(
                "api-key",
                CheckStatus::Ok,
                "the API key signs in as an Immich administrator",
            );
            match found {
                Some(l) => l,
                None if out.candidates.is_empty() => {
                    out.push(
                        "library",
                        CheckStatus::Fail,
                        "Immich has no external library yet: create one (Administration → \
                         External libraries) that imports this folder",
                    );
                    return out;
                }
                None => {
                    let holding = out
                        .candidates
                        .iter()
                        .any(|l| l.import_paths.iter().any(|p| covers(p, import_path)));
                    let message = if holding {
                        format!(
                            "{} libraries import {import_path}: choose one ({})",
                            out.candidates.len(),
                            describe(&out.candidates)
                        )
                    } else {
                        format!(
                            "no external library imports {import_path}; Immich has {}",
                            describe(&out.candidates)
                        )
                    };
                    out.push("library", CheckStatus::Fail, message);
                    return out;
                }
            }
        }
        Err(e) => {
            match e.kind {
                ApiErrorKind::Auth => out.push(
                    "api-key",
                    CheckStatus::Fail,
                    format!("Immich refused the API key ({})", e.message),
                ),
                ApiErrorKind::Forbidden => out.push(
                    "api-key",
                    CheckStatus::Fail,
                    format!(
                        "the API key may not read the libraries: its user must be an \
                         administrator, and the key needs library.read and library.update \
                         ({})",
                        e.message
                    ),
                ),
                ApiErrorKind::NotFound => {
                    out.push(
                        "api-key",
                        CheckStatus::Ok,
                        "the API key signs in as an Immich administrator",
                    );
                    out.push(
                        "library",
                        CheckStatus::Fail,
                        format!(
                            "Immich has no library {} ({})",
                            library_id.unwrap_or_default(),
                            e.message
                        ),
                    );
                }
                _ => out.push("server", CheckStatus::Fail, e.message),
            }
            return out;
        }
    };
    out.push(
        "library",
        CheckStatus::Ok,
        format!(
            "library {} ({} assets) imports {}",
            library.name,
            library.asset_count,
            library.import_paths.join(", ")
        ),
    );

    let Some(root) = library
        .import_paths
        .iter()
        .find(|p| covers(p, import_path))
        .cloned()
    else {
        out.push(
            "import-path",
            CheckStatus::Fail,
            format!(
                "{import_path} is outside the library {}, which imports {}: send to one of \
                 those folders, or add it to the library in Immich",
                library.name,
                library.import_paths.join(", ")
            ),
        );
        out.library = Some(library);
        return out;
    };
    out.push(
        "import-path",
        CheckStatus::Ok,
        if root.trim_end_matches('/') == import_path {
            format!("the destination is the library's folder {root}")
        } else {
            format!("the destination {import_path} is inside the library's folder {root}")
        },
    );

    // Whether Immich itself can read the folder it imports from. A warning only: the files
    // still arrive safely, and an older Immich may not answer this.
    match api
        .validate(server, key, &library.id, std::slice::from_ref(&root))
        .await
    {
        Ok(v) => match v.iter().find(|p| p.import_path == root) {
            Some(p) if p.is_valid => out.push(
                "readable",
                CheckStatus::Ok,
                format!("Immich can read {root}"),
            ),
            Some(p) => out.push(
                "readable",
                CheckStatus::Warn,
                format!(
                    "Immich can't read {root}: {}",
                    p.message.as_deref().unwrap_or("no reason given")
                ),
            ),
            None => out.push(
                "readable",
                CheckStatus::Warn,
                format!("Immich said nothing about {root}"),
            ),
        },
        Err(e) => out.push(
            "readable",
            CheckStatus::Warn,
            format!(
                "could not ask Immich whether it can read {root}: {}",
                e.message
            ),
        ),
    }
    out.import_root = Some(root);
    out.library = Some(library);
    out
}
