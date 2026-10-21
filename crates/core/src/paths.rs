//! Final destination paths, computed when a job starts: the analysed paths plus the user's
//! decisions (edit, keep original, skip, accept a collision suffix).

use std::collections::{HashMap, HashSet};

use filebridge_store::{Decision, IssueRow};

use crate::analyse::{RULE_COLLISION, RULE_UNDECODABLE};
use crate::{CoreError, Result};

/// One manifest entry, as needed for planning.
pub(crate) struct Node {
    pub parent: Option<u64>,
    pub raw_path: Vec<u8>,
    pub dest_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Override {
    Name(String),
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Planned {
    Path(String),
    Skipped,
}

fn last(bytes: &[u8]) -> &[u8] {
    bytes.rsplit(|b| *b == b'/').next().unwrap_or(bytes)
}

/// Turns decided issues into per-entry overrides. Fails while any decision is pending.
pub(crate) fn overrides(nodes: &[Node], issues: &[IssueRow]) -> Result<HashMap<u64, Override>> {
    let pending = issues
        .iter()
        .filter(|i| i.issue.decision == Decision::Pending)
        .count();
    if pending > 0 {
        return Err(CoreError::PendingDecisions(pending as u64));
    }
    let mut out = HashMap::new();
    for row in issues {
        let idx = row.issue.entry_idx;
        let raw = || {
            nodes
                .get(idx as usize)
                .map(|n| last(&n.raw_path).to_vec())
                .unwrap_or_default()
        };
        let ov = match row.issue.decision {
            Decision::Pending => unreachable!("checked above"),
            Decision::Skipped => Override::Skip,
            Decision::Edited => Override::Name(row.custom_name.clone().unwrap_or_default()),
            Decision::Rejected => Override::Name(String::from_utf8(raw()).map_err(|_| {
                CoreError::Invalid(format!(
                    "issue {}: the original name is not valid UTF-8; edit or skip it",
                    row.id
                ))
            })?),
            Decision::Accepted if row.issue.rule == RULE_COLLISION => {
                Override::Name(String::from_utf8_lossy(&row.issue.after).into_owned())
            }
            Decision::Accepted if row.issue.rule == RULE_UNDECODABLE => {
                return Err(CoreError::Invalid(format!(
                    "issue {}: an undecodable name has no proposal; edit or skip it",
                    row.id
                )));
            }
            Decision::Accepted => continue, // already applied by the analysis
        };
        if out.get(&idx) != Some(&Override::Skip) {
            out.insert(idx, ov);
        }
    }
    Ok(out)
}

/// Destination path (or skip) for every node, in order. Nodes are in scanner order: parents
/// before children, `nodes[i]` has idx `i`.
pub(crate) fn plan(
    nodes: &[Node],
    overrides: &HashMap<u64, Override>,
    case_sensitive: bool,
) -> Result<Vec<Planned>> {
    let mut out: Vec<Planned> = Vec::with_capacity(nodes.len());
    let mut seen = HashSet::new();
    for (i, node) in nodes.iter().enumerate() {
        let idx = i as u64;
        let parent = match node.parent {
            None => None,
            Some(p) => match out.get(p as usize) {
                Some(Planned::Skipped) => {
                    out.push(Planned::Skipped);
                    continue;
                }
                Some(Planned::Path(pp)) => Some(pp.clone()),
                None => {
                    return Err(CoreError::Invalid(format!(
                        "entry {idx}: parent after child"
                    )));
                }
            },
        };
        let name = match overrides.get(&idx) {
            Some(Override::Skip) => {
                out.push(Planned::Skipped);
                continue;
            }
            Some(Override::Name(n)) => n.clone(),
            None => match &node.dest_path {
                Some(p) => p.rsplit('/').next().unwrap_or(p).to_owned(),
                None => {
                    return Err(CoreError::Invalid(format!(
                        "no destination name for {}",
                        String::from_utf8_lossy(&node.raw_path)
                    )));
                }
            },
        };
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(CoreError::Invalid(format!(
                "invalid destination name {name:?}"
            )));
        }
        let path = parent.map_or(name.clone(), |pp| format!("{pp}/{name}"));
        let key = if case_sensitive {
            path.clone()
        } else {
            path.to_lowercase()
        };
        if !seen.insert(key) {
            return Err(CoreError::Invalid(format!(
                "two entries would be sent to {path:?}; rename or skip one"
            )));
        }
        out.push(Planned::Path(path));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use filebridge_store::NewIssue;

    fn node(parent: Option<u64>, raw: &str, dest: Option<&str>) -> Node {
        Node {
            parent,
            raw_path: raw.as_bytes().to_vec(),
            dest_path: dest.map(str::to_owned),
        }
    }

    fn issue(
        idx: u64,
        rule: &str,
        after: &str,
        decision: Decision,
        custom: Option<&str>,
    ) -> IssueRow {
        IssueRow {
            id: idx as i64,
            issue: NewIssue {
                entry_idx: idx,
                rule: rule.into(),
                severity: "needs_decision".into(),
                message_key: String::new(),
                before: Vec::new(),
                after: after.as_bytes().to_vec(),
                decision,
            },
            custom_name: custom.map(str::to_owned),
            decided_at: None,
        }
    }

    fn tree() -> Vec<Node> {
        vec![
            node(None, "Acme", Some("Acme")),
            node(Some(0), "Acme/dir", Some("Acme/dir")),
            node(Some(1), "Acme/dir/a.txt", Some("Acme/dir/a.txt")),
            node(Some(0), "Acme/b.txt", Some("Acme/b.txt")),
            node(Some(0), "Acme/b2.txt", Some("Acme/b.txt")), // collides
        ]
    }

    #[test]
    fn pending_blocks_and_collision_suffix_applies() {
        let nodes = tree();
        let pending = [issue(
            4,
            RULE_COLLISION,
            "b (2).txt",
            Decision::Pending,
            None,
        )];
        assert!(matches!(
            overrides(&nodes, &pending),
            Err(CoreError::PendingDecisions(1))
        ));
        let accepted = [issue(
            4,
            RULE_COLLISION,
            "b (2).txt",
            Decision::Accepted,
            None,
        )];
        let ov = overrides(&nodes, &accepted).unwrap();
        let planned = plan(&nodes, &ov, true).unwrap();
        assert_eq!(planned[4], Planned::Path("Acme/b (2).txt".into()));
        assert!(
            plan(&nodes, &HashMap::new(), true).is_err(),
            "unresolved collision"
        );
    }

    #[test]
    fn skip_and_edit_propagate_to_children() {
        let nodes = tree();
        let issues = [
            issue(1, "nfc", "", Decision::Edited, Some("Pasta")),
            issue(4, RULE_COLLISION, "b (2).txt", Decision::Skipped, None),
        ];
        let planned = plan(&nodes, &overrides(&nodes, &issues).unwrap(), true).unwrap();
        assert_eq!(planned[2], Planned::Path("Acme/Pasta/a.txt".into()));
        assert_eq!(planned[4], Planned::Skipped);

        let skip_dir = [
            issue(1, "nfc", "", Decision::Skipped, None),
            issue(4, RULE_COLLISION, "b (2).txt", Decision::Accepted, None),
        ];
        let planned = plan(&nodes, &overrides(&nodes, &skip_dir).unwrap(), true).unwrap();
        assert_eq!(planned[1], Planned::Skipped);
        assert_eq!(planned[2], Planned::Skipped);
    }

    #[test]
    fn rejected_keeps_the_original_name() {
        let nodes = vec![node(None, "Vera\u{303}o", Some("Verão"))];
        let issues = [issue(0, "nfc", "Verão", Decision::Rejected, None)];
        let planned = plan(&nodes, &overrides(&nodes, &issues).unwrap(), true).unwrap();
        assert_eq!(planned[0], Planned::Path("Vera\u{303}o".into()));
    }

    #[test]
    fn invalid_edited_names_are_refused() {
        let nodes = vec![node(None, "x", Some("x"))];
        let issues = [issue(0, "nfc", "", Decision::Edited, Some("a/b"))];
        assert!(plan(&nodes, &overrides(&nodes, &issues).unwrap(), true).is_err());
    }
}
