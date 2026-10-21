//! Name issues found in stage 3 and the user's decisions on them. Decisions are persisted per
//! issue, so a resumed job never asks twice (docs/04, "Decisions model").

use rusqlite::{Row, params};

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
}

/// Per-rule counts for the Analysis step ("312 names with decomposed accents").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSummary {
    pub rule: String,
    pub total: u64,
    pub pending: u64,
}

const COLUMNS: &str =
    "id, entry_idx, rule, severity, message_key, before, after, decision, custom_name, decided_at";

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
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {COLUMNS} FROM issues
                 WHERE job_id = ?1 AND (?2 IS NULL OR rule = ?2) AND (?3 IS NULL OR id > ?3)
                 ORDER BY id LIMIT ?4"
            ))?;
            let rows = stmt.query_map(params![job_id, rule, after_id, limit], from_row)?;
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
        let job_id = job_id.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare_cached(
                "SELECT rule, COUNT(*), SUM(decision = 'pending')
                 FROM issues WHERE job_id = ?1 GROUP BY rule ORDER BY rule",
            )?;
            let rows = stmt.query_map([&job_id], |r| {
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

    /// Records a decision. `Edited` requires `custom_name`; other decisions clear it.
    pub async fn decide_issue(
        &self,
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
        self.call(move |c| {
            let n = c.execute(
                "UPDATE issues SET decision = ?2, custom_name = ?3, decided_at = ?4 WHERE id = ?1",
                params![issue_id, decision.as_str(), custom_name, now_ms()],
            )?;
            if n == 0 {
                return Err(StoreError::NotFound(format!("issue {issue_id}")));
            }
            Ok(())
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
            store.decide_issue(moji[0].id, Decision::Edited, None).await,
            Err(StoreError::Invalid(_))
        ));
        store
            .decide_issue(moji[0].id, Decision::Edited, Some("Patológica".into()))
            .await
            .unwrap();
        store
            .decide_issue(moji[1].id, Decision::Skipped, Some("ignored".into()))
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
}
