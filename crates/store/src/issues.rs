//! Name issues found in stage 3 and the user's decisions on them. Decisions are persisted per
//! issue, so a resumed job never asks twice (docs/04, "Decisions model").

use rusqlite::{OptionalExtension, Row, params};

use crate::{Result, Store, StoreError, now_ms};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Decision {
    /// Waiting for the user; blocks Send.
    Pending,
    /// Use the rule's proposal (the default for automatic rules).
    Accepted,
    /// Use the user's own name (`custom_name`).
    Edited,
    /// Keep the original name.
    Rejected,
    /// Leave the file out of the send.
    Skipped,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Edited => "edited",
            Self::Rejected => "rejected",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "pending" => Self::Pending,
            "accepted" => Self::Accepted,
            "edited" => Self::Edited,
            "rejected" => Self::Rejected,
            "skipped" => Self::Skipped,
            other => return Err(StoreError::Invalid(format!("decision {other:?}"))),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewIssue {
    pub entry_idx: u64,
    pub rule: String,
    /// `names::Severity` as text (`auto` / `needs_decision`).
    pub severity: String,
    pub message_key: String,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
    /// `Accepted` for automatic rules, `Pending` for those that need a decision.
    pub decision: Decision,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IssueRow {
    pub id: i64,
    pub issue: NewIssue,
    pub custom_name: Option<String>,
    pub decided_at: Option<i64>,
    /// The decision was copied from the job this one repeats (Send again, D06).
    pub carried_over: bool,
}

/// Per-rule counts for the Analysis step ("312 names with decomposed accents").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSummary {
    pub rule: String,
    pub total: u64,
    pub pending: u64,
}

/// The condition "the issue's entry's source path is LIKE parameter `?n`" (true when NULL).
fn matching(n: u8) -> String {
    format!(
        "(?{n} IS NULL OR EXISTS (SELECT 1 FROM manifest_entries m WHERE m.job_id = issues.job_id \
         AND m.idx = issues.entry_idx AND CAST(m.raw_path AS TEXT) LIKE ?{n} ESCAPE '\\'))"
    )
}

/// A substring search as a LIKE pattern, `%`/`_` literal; `None` for no filter.
fn like_pattern(query: Option<String>) -> Option<String> {
    query
        .map(|q| q.trim().to_owned())
        .filter(|q| !q.is_empty())
        .map(|q| format!("%{}%", crate::manifest::like_escape(&q)))
}

const COLUMNS: &str = "id, entry_idx, rule, severity, message_key, before, after, decision, \
                       custom_name, decided_at, carried_over";

fn from_row(row: &Row<'_>) -> rusqlite::Result<(IssueRow, String)> {
    Ok((
        IssueRow {
            id: row.get(0)?,
            issue: NewIssue {
                entry_idx: row.get(1)?,
                rule: row.get(2)?,
                severity: row.get(3)?,
                message_key: row.get(4)?,
                before: row.get(5)?,
                after: row.get(6)?,
                decision: Decision::Pending,
            },
            custom_name: row.get(8)?,
            decided_at: row.get(9)?,
            carried_over: row.get(10)?,
        },
        row.get(7)?,
    ))
}

impl Store {
    pub async fn insert_issues(&self, job_id: &str, issues: Vec<NewIssue>) -> Result<()> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            {
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO issues
                         (job_id, entry_idx, rule, severity, message_key, before, after, decision)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?;
                for i in &issues {
                    stmt.execute(params![
                        job_id,
                        i.entry_idx,
                        i.rule,
                        i.severity,
                        i.message_key,
                        i.before,
                        i.after,
                        i.decision.as_str()
                    ])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Drops every issue of a job, before re-running the analysis.
    pub async fn clear_issues(&self, job_id: &str) -> Result<()> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            c.execute("DELETE FROM issues WHERE job_id = ?1", [&job_id])?;
            Ok(())
        })
        .await
    }

    /// Issues of a job, optionally of one rule, by id after `after_id` (keyset pagination).
    pub async fn issues_page(
        &self,
        job_id: &str,
        rule: Option<String>,
        after_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<IssueRow>> {
        self.issues_page_matching(job_id, rule, after_id, limit, None)
            .await
    }

    /// [`Self::issues_page`], only issues whose entry's source path contains `query`
    /// (docs/design/specs/D08).
    pub async fn issues_page_matching(
        &self,
        job_id: &str,
        rule: Option<String>,
        after_id: Option<i64>,
        limit: u32,
        query: Option<String>,
    ) -> Result<Vec<IssueRow>> {
        let job_id = job_id.to_owned();
        let pattern = like_pattern(query);
        self.call(move |c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM issues
                 WHERE job_id = ?1 AND (?2 IS NULL OR rule = ?2) AND (?3 IS NULL OR id > ?3)
                   AND {}
                 ORDER BY id LIMIT ?4",
                matching(5)
            ))?;
            let rows = stmt.query_map(params![job_id, rule, after_id, limit, pattern], from_row)?;
            rows.map(|r| {
                let (mut row, decision) = r?;
                row.issue.decision = Decision::parse(&decision)?;
                Ok(row)
            })
            .collect()
        })
        .await
    }

    pub async fn issue_summary(&self, job_id: &str) -> Result<Vec<RuleSummary>> {
        self.issue_summary_matching(job_id, None).await
    }

    /// Per-rule counts of the issues whose entry's source path contains `query`.
    pub async fn issue_summary_matching(
        &self,
        job_id: &str,
        query: Option<String>,
    ) -> Result<Vec<RuleSummary>> {
        let job_id = job_id.to_owned();
        let pattern = like_pattern(query);
        self.call(move |c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT rule, COUNT(*), SUM(decision = 'pending')
                 FROM issues WHERE job_id = ?1 AND {} GROUP BY rule ORDER BY rule",
                matching(2)
            ))?;
            let rows = stmt.query_map(params![job_id, pattern], |r| {
                Ok(RuleSummary {
                    rule: r.get(0)?,
                    total: r.get(1)?,
                    pending: r.get(2)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })
        .await
    }

    pub async fn pending_issue_count(&self, job_id: &str) -> Result<u64> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM issues WHERE job_id = ?1 AND decision = 'pending'",
                [&job_id],
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// One issue of a job. `NotFound` if the id belongs to another job.
    pub async fn issue(&self, job_id: &str, issue_id: i64) -> Result<IssueRow> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let found = c
                .query_row(
                    &format!("SELECT {COLUMNS} FROM issues WHERE job_id = ?1 AND id = ?2"),
                    params![job_id, issue_id],
                    from_row,
                )
                .optional()?;
            let Some((mut row, decision)) = found else {
                return Err(StoreError::NotFound(format!("issue {issue_id}")));
            };
            row.issue.decision = Decision::parse(&decision)?;
            Ok(row)
        })
        .await
    }

    /// Copies the decisions of `from` (the job `to` repeats) onto `to`'s issues of the same
    /// entry (same source and raw path) and rule, where `from` had decided and `to` differs.
    /// Marks them `carried_over`. Returns how many changed.
    pub async fn copy_decisions(&self, from: &str, to: &str) -> Result<u64> {
        let (from, to) = (from.to_owned(), to.to_owned());
        self.call(move |c| {
            let n = c.execute(
                "UPDATE issues SET decision = o.decision, custom_name = o.custom_name,
                     decided_at = ?3, carried_over = 1
                 FROM issues o
                 JOIN manifest_entries om ON om.job_id = o.job_id AND om.idx = o.entry_idx
                 JOIN manifest_entries nm ON nm.job_id = ?2
                 WHERE issues.job_id = ?2 AND o.job_id = ?1
                   AND nm.idx = issues.entry_idx
                   AND om.source = nm.source AND om.raw_path = nm.raw_path
                   AND o.rule = issues.rule
                   AND o.decision <> 'pending'
                   AND (issues.decision <> o.decision OR issues.custom_name IS NOT o.custom_name)",
                params![from, to, now_ms()],
            )?;
            Ok(n as u64)
        })
        .await
    }

    /// Puts carried-over decisions back to pending (they don't fit the new destination).
    pub async fn reset_issues(&self, job_id: &str, ids: Vec<i64>) -> Result<()> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            {
                let mut stmt = tx.prepare_cached(
                    "UPDATE issues SET decision = 'pending', custom_name = NULL, decided_at = NULL,
                         carried_over = 0
                     WHERE job_id = ?1 AND id = ?2",
                )?;
                for id in &ids {
                    stmt.execute(params![job_id, id])?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// For each entry in `idxs` that a rule renamed (accepted or edited issue), those rules.
    pub async fn renaming_rules(
        &self,
        job_id: &str,
        idxs: Vec<u64>,
    ) -> Result<std::collections::HashMap<u64, Vec<String>>> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let mut out: std::collections::HashMap<u64, Vec<String>> = Default::default();
            let mut stmt = c.prepare_cached(
                "SELECT entry_idx, rule FROM issues
                 WHERE job_id = ?1 AND entry_idx = ?2 AND decision IN ('accepted', 'edited')
                 ORDER BY id",
            )?;
            for idx in idxs {
                let rows = stmt.query_map(params![job_id, idx], |r| Ok((r.get(0)?, r.get(1)?)))?;
                for row in rows {
                    let (i, rule): (u64, String) = row?;
                    out.entry(i).or_default().push(rule);
                }
            }
            Ok(out)
        })
        .await
    }

    /// The issues of one entry, in id order.
    pub async fn issues_for_entry(&self, job_id: &str, entry_idx: u64) -> Result<Vec<IssueRow>> {
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM issues WHERE job_id = ?1 AND entry_idx = ?2 ORDER BY id"
            ))?;
            let rows = stmt.query_map(params![job_id, entry_idx], from_row)?;
            rows.map(|r| {
                let (mut row, decision) = r?;
                row.issue.decision = Decision::parse(&decision)?;
                Ok(row)
            })
            .collect()
        })
        .await
    }

    /// Records a decision on one issue of a job. `Edited` requires `custom_name`; other
    /// decisions clear it. `NotFound` if the issue belongs to another job.
    pub async fn decide_issue(
        &self,
        job_id: &str,
        issue_id: i64,
        decision: Decision,
        custom_name: Option<String>,
    ) -> Result<()> {
        match (decision, &custom_name) {
            (Decision::Edited, None) => {
                return Err(StoreError::Invalid(
                    "an edited name needs custom_name".into(),
                ));
            }
            (Decision::Pending, _) => {
                return Err(StoreError::Invalid(
                    "cannot decide an issue back to pending".into(),
                ));
            }
            _ => {}
        }
        let custom_name = custom_name.filter(|_| decision == Decision::Edited);
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE issues SET decision = ?3, custom_name = ?4, decided_at = ?5
                 WHERE job_id = ?1 AND id = ?2",
                params![job_id, issue_id, decision.as_str(), custom_name, now_ms()],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(format!("issue {issue_id}")));
            }
            Ok(())
        })
        .await
    }

    /// Records one decision for every pending issue of a job (optionally of one rule), in a
    /// single statement. Decided issues are left alone. `Edited` is refused: a custom name
    /// belongs to one file. Returns how many issues changed.
    pub async fn decide_pending_issues(
        &self,
        job_id: &str,
        rule: Option<String>,
        decision: Decision,
    ) -> Result<u64> {
        self.decide_pending_issues_matching(job_id, rule, decision, None)
            .await
    }

    /// [`Self::decide_pending_issues`], only for issues whose entry's source path contains
    /// `query` (docs/design/specs/D08, "Select the N that match").
    pub async fn decide_pending_issues_matching(
        &self,
        job_id: &str,
        rule: Option<String>,
        decision: Decision,
        query: Option<String>,
    ) -> Result<u64> {
        if matches!(decision, Decision::Pending | Decision::Edited) {
            return Err(StoreError::Invalid(format!(
                "cannot decide issues in bulk as {}",
                decision.as_str()
            )));
        }
        let job_id = job_id.to_owned();
        let pattern = like_pattern(query);
        self.call(move |c| {
            let n = c.execute(
                &format!(
                    "UPDATE issues SET decision = ?3, custom_name = NULL, decided_at = ?4,
                         carried_over = 0
                     WHERE job_id = ?1 AND decision = 'pending' AND (?2 IS NULL OR rule = ?2)
                       AND {}",
                    matching(5)
                ),
                params![job_id, rule, decision.as_str(), now_ms(), pattern],
            )?;
            Ok(n as u64)
        })
        .await
    }

    /// One decision for each listed issue of the job (not `Edited`), in one transaction.
    /// Issues of another job are ignored. Returns how many changed.
    pub async fn decide_issues(
        &self,
        job_id: &str,
        ids: Vec<i64>,
        decision: Decision,
    ) -> Result<u64> {
        if matches!(decision, Decision::Pending | Decision::Edited) {
            return Err(StoreError::Invalid(format!(
                "cannot decide issues in bulk as {}",
                decision.as_str()
            )));
        }
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            let mut n = 0;
            {
                let mut stmt = tx.prepare_cached(
                    "UPDATE issues SET decision = ?3, custom_name = NULL, decided_at = ?4,
                         carried_over = 0
                     WHERE job_id = ?1 AND id = ?2",
                )?;
                let now = now_ms();
                for id in &ids {
                    n += stmt.execute(params![job_id, id, decision.as_str(), now])?;
                }
            }
            tx.commit()?;
            Ok(n as u64)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::store_with_job;

    fn issue(idx: u64, rule: &str, decision: Decision) -> NewIssue {
        NewIssue {
            entry_idx: idx,
            rule: rule.into(),
            severity: if decision == Decision::Pending {
                "needs_decision"
            } else {
                "auto"
            }
            .into(),
            message_key: format!("issue.{rule}"),
            before: b"Vera\xcc\x83o".to_vec(),
            after: "Verão".as_bytes().to_vec(),
            decision,
        }
    }

    #[tokio::test]
    async fn summary_decisions_and_pages() {
        let (store, job) = store_with_job().await;
        let mut issues: Vec<_> = (0..5)
            .map(|i| issue(i, "nfc", Decision::Accepted))
            .collect();
        issues.push(issue(5, "mojibake", Decision::Pending));
        issues.push(issue(6, "mojibake", Decision::Pending));
        store.insert_issues(&job, issues).await.unwrap();

        let summary = store.issue_summary(&job).await.unwrap();
        assert_eq!(
            summary,
            vec![
                RuleSummary {
                    rule: "mojibake".into(),
                    total: 2,
                    pending: 2
                },
                RuleSummary {
                    rule: "nfc".into(),
                    total: 5,
                    pending: 0
                },
            ]
        );
        assert_eq!(store.pending_issue_count(&job).await.unwrap(), 2);

        let moji = store
            .issues_page(&job, Some("mojibake".into()), None, 10)
            .await
            .unwrap();
        assert_eq!(moji.len(), 2);
        assert_eq!(moji[0].issue.before, b"Vera\xcc\x83o");

        assert!(matches!(
            store
                .decide_issue(&job, moji[0].id, Decision::Edited, None)
                .await,
            Err(StoreError::Invalid(_))
        ));
        store
            .decide_issue(
                &job,
                moji[0].id,
                Decision::Edited,
                Some("Patológica".into()),
            )
            .await
            .unwrap();
        store
            .decide_issue(&job, moji[1].id, Decision::Skipped, Some("ignored".into()))
            .await
            .unwrap();
        assert_eq!(store.pending_issue_count(&job).await.unwrap(), 0);

        let moji = store
            .issues_page(&job, Some("mojibake".into()), None, 10)
            .await
            .unwrap();
        assert_eq!(moji[0].issue.decision, Decision::Edited);
        assert_eq!(moji[0].custom_name.as_deref(), Some("Patológica"));
        assert_eq!(moji[1].custom_name, None);
        assert!(moji[1].decided_at.is_some());

        let page = store
            .issues_page(&job, None, Some(moji[0].id), 10)
            .await
            .unwrap();
        assert_eq!(page.len(), 1);

        store.clear_issues(&job).await.unwrap();
        assert!(store.issue_summary(&job).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn bulk_decisions_touch_only_pending_issues() {
        let (store, job) = store_with_job().await;
        store
            .insert_issues(
                &job,
                vec![
                    issue(0, "nfc", Decision::Accepted),
                    issue(1, "collision", Decision::Pending),
                    issue(2, "collision", Decision::Pending),
                    issue(3, "mojibake", Decision::Pending),
                ],
            )
            .await
            .unwrap();
        let first = store
            .issues_page(&job, Some("collision".into()), None, 1)
            .await
            .unwrap();
        store
            .decide_issue(&job, first[0].id, Decision::Edited, Some("mine".into()))
            .await
            .unwrap();

        for refused in [Decision::Edited, Decision::Pending] {
            assert!(matches!(
                store.decide_pending_issues(&job, None, refused).await,
                Err(StoreError::Invalid(_))
            ));
        }
        let n = store
            .decide_pending_issues(&job, Some("collision".into()), Decision::Skipped)
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(store.pending_issue_count(&job).await.unwrap(), 1);

        let rows = store.issues_page(&job, None, None, 10).await.unwrap();
        let decisions: Vec<_> = rows.iter().map(|r| r.issue.decision).collect();
        assert_eq!(
            decisions,
            [
                Decision::Accepted,
                Decision::Edited,
                Decision::Skipped,
                Decision::Pending
            ]
        );
        assert_eq!(rows[1].custom_name.as_deref(), Some("mine"));
        assert!(rows[2].decided_at.is_some());

        let n = store
            .decide_pending_issues(&job, None, Decision::Accepted)
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(store.pending_issue_count(&job).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn an_issue_is_only_reached_through_its_own_job() {
        let (store, job) = store_with_job().await;
        store
            .insert_issues(&job, vec![issue(0, "collision", Decision::Pending)])
            .await
            .unwrap();
        let id = store.issues_page(&job, None, None, 1).await.unwrap()[0].id;

        let other = "another-job";
        assert!(matches!(
            store.issue(other, id).await,
            Err(StoreError::NotFound(_))
        ));
        assert!(matches!(
            store.decide_issue(other, id, Decision::Skipped, None).await,
            Err(StoreError::NotFound(_))
        ));
        assert_eq!(
            store.issue(&job, id).await.unwrap().issue.decision,
            Decision::Pending
        );
        store
            .decide_issue(&job, id, Decision::Skipped, None)
            .await
            .unwrap();
        assert_eq!(
            store.issue(&job, id).await.unwrap().issue.decision,
            Decision::Skipped
        );
    }
}
