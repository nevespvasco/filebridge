//! Embedded migrations, tracked with `PRAGMA user_version`. Append new migrations; never edit one
//! that has shipped.

use rusqlite::Connection;

use crate::{Result, StoreError};

const MIGRATIONS: &[&str] = &[
    // 1: initial schema (docs/02-architecture.md, "Persistence model").
    r#"
CREATE TABLE profiles (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    transport   TEXT NOT NULL,
    integration TEXT,
    -- JSON: destination and integration settings. Keyring references only, never secrets.
    config      TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE jobs (
    id            TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    profile_id    TEXT NOT NULL REFERENCES profiles(id),
    state         TEXT NOT NULL,
    options       TEXT NOT NULL,
    error         TEXT,
    attempt       INTEGER NOT NULL DEFAULT 0,
    next_retry_at INTEGER,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL,
    finished_at   INTEGER
);
CREATE INDEX jobs_unfinished ON jobs(finished_at, created_at);

CREATE TABLE job_sources (
    job_id   TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    path     BLOB NOT NULL,
    PRIMARY KEY (job_id, position)
);

CREATE TABLE manifest_entries (
    job_id         TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    idx            INTEGER NOT NULL,
    source         INTEGER NOT NULL,
    raw_path       BLOB NOT NULL,
    parent_idx     INTEGER,
    is_dir         INTEGER NOT NULL,
    size           INTEGER NOT NULL,
    mtime          INTEGER,
    hash           BLOB,
    hash_algo      TEXT,
    dest_path      TEXT,
    transfer_state TEXT NOT NULL DEFAULT 'pending',
    verify_state   TEXT NOT NULL DEFAULT 'pending',
    error          TEXT,
    PRIMARY KEY (job_id, idx)
) WITHOUT ROWID;
CREATE INDEX manifest_transfer_state ON manifest_entries(job_id, transfer_state);

CREATE TABLE issues (
    id          INTEGER PRIMARY KEY,
    job_id      TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    entry_idx   INTEGER NOT NULL,
    rule        TEXT NOT NULL,
    severity    TEXT NOT NULL,
    message_key TEXT NOT NULL,
    before      BLOB NOT NULL,
    after       BLOB NOT NULL,
    decision    TEXT NOT NULL,
    custom_name TEXT,
    decided_at  INTEGER
);
CREATE INDEX issues_job_rule ON issues(job_id, rule, id);
CREATE INDEX issues_job_decision ON issues(job_id, decision);

CREATE TABLE events (
    id     INTEGER PRIMARY KEY,
    job_id TEXT REFERENCES jobs(id) ON DELETE CASCADE,
    at     INTEGER NOT NULL,
    kind   TEXT NOT NULL,
    data   TEXT NOT NULL
);
CREATE INDEX events_job ON events(job_id, id);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#,
    // 2: what a job's finished stages produced, saved with each state change so a resumed job
    // ends with the warnings and verify rounds it had before the restart.
    r#"
ALTER TABLE jobs ADD COLUMN warnings TEXT NOT NULL DEFAULT '[]';
ALTER TABLE jobs ADD COLUMN verify_rounds INTEGER NOT NULL DEFAULT 0;
"#,
    // 3: each file's Category (docs/design/specs/D02), set at analysis. Older jobs keep 'other'.
    r#"
ALTER TABLE manifest_entries ADD COLUMN category TEXT NOT NULL DEFAULT 'other';
CREATE INDEX manifest_category ON manifest_entries(job_id, category);
"#,
    // 4: the Files tab's filters and orders, the issues of one entry, and Activity's milestones
    // (docs/design/specs/D05, D07).
    r#"
CREATE INDEX manifest_verify_state ON manifest_entries(job_id, verify_state);
CREATE INDEX manifest_size ON manifest_entries(job_id, size, idx);
CREATE INDEX issues_job_entry ON issues(job_id, entry_idx);
CREATE INDEX events_kind ON events(kind, id);
"#,
    // 5: each profile's last connection test, so its space shows without a network call
    // (docs/design/specs/D03). JSON of the preflight views: no secrets.
    r#"
ALTER TABLE profiles ADD COLUMN last_test TEXT;
ALTER TABLE profiles ADD COLUMN last_test_at INTEGER;
ALTER TABLE profiles ADD COLUMN last_good_test TEXT;
ALTER TABLE profiles ADD COLUMN last_good_test_at INTEGER;
"#,
    // 6: a job's name, the job it repeats (Send again), decisions carried from it, and what the
    // reclaimable-source check found (docs/design/specs/D06, D10).
    r#"
ALTER TABLE jobs ADD COLUMN label TEXT;
ALTER TABLE jobs ADD COLUMN again_of TEXT REFERENCES jobs(id) ON DELETE SET NULL;
ALTER TABLE jobs ADD COLUMN reclaim_dismissed_at INTEGER;
ALTER TABLE jobs ADD COLUMN reclaim_check TEXT;
ALTER TABLE issues ADD COLUMN carried_over INTEGER NOT NULL DEFAULT 0;
CREATE INDEX jobs_state ON jobs(state, created_at);
CREATE INDEX jobs_profile ON jobs(profile_id, created_at);
"#,
    // 7: a deleted destination is forgotten but its row stays, so the transfers that used it
    // keep their history (the jobs reference it).
    r#"
ALTER TABLE profiles ADD COLUMN deleted_at INTEGER;
"#,
    // 8: what the job's error is, when the UI has its own words for it (`destination_full`),
    // next to the message itself (docs/design/specs/D03).
    r#"
ALTER TABLE jobs ADD COLUMN error_code TEXT;
"#,
    // 9: the folder under the profile root that the last transfer to it was sent into (NULL =
    // the root), which the wizard preselects next time (docs/design/specs/D11).
    r#"
ALTER TABLE profiles ADD COLUMN last_target_dir TEXT;
"#,
    // 10: folders and files pinned in step 1, to add them again in one click
    // (docs/design/specs/D02). The path as the user picked it.
    r#"
CREATE TABLE pinned_sources (
    path      TEXT PRIMARY KEY,
    pinned_at INTEGER NOT NULL
);
"#,
];

pub fn migrate(conn: &mut Connection) -> Result<()> {
    let current: usize = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if current > MIGRATIONS.len() {
        return Err(StoreError::Invalid(format!(
            "database schema version {current} is newer than this app ({})",
            MIGRATIONS.len()
        )));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i + 1)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let v: usize = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len());
    }

    #[test]
    fn refuses_a_newer_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", MIGRATIONS.len() + 1)
            .unwrap();
        assert!(matches!(migrate(&mut conn), Err(StoreError::Invalid(_))));
    }
}
