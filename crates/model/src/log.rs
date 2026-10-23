//! The job log as the UI reads it (docs/design/specs/D07-job-log-and-activity.md): what the
//! engine recorded about a job, one row per milestone, with parameters but no sentences, so an
//! old log reads in the user's current language.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// One line of a job's log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct LogEntry {
    pub id: i64,
    /// Unix ms.
    pub at: i64,
    /// What happened (`created`, `state`, `retry`, `transfer_round`, `verify`, `finished`, …);
    /// the UI turns it into a sentence. A kind it doesn't know is shown neutrally.
    pub kind: String,
    pub level: LogLevel,
    /// The sentence's parameters. Never secrets.
    pub data: Value,
}

/// A milestone of any job, or a profile change, for Activity (docs/design/specs/D07).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ActivityItem {
    pub id: i64,
    pub at: i64,
    pub kind: String,
    pub level: LogLevel,
    pub job_id: Option<String>,
    /// The job's name, or its first source's name.
    pub job_label: Option<String>,
    /// The destination's name (for a profile change, the profile's).
    pub profile_name: Option<String>,
    pub data: Value,
}

/// How a job is called when it has no name: its first source's name, "+N" for the others.
pub fn default_label(sources: &[String]) -> String {
    let Some(first) = sources.first() else {
        return "—".into();
    };
    let name = first
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(first);
    if sources.len() > 1 {
        format!("{name} +{}", sources.len() - 1)
    } else {
        name.to_owned()
    }
}

/// How serious a recorded event is, from its kind and parameters. Derived when read, not
/// stored, so the rule can change without touching old logs.
pub fn log_level(kind: &str, data: &Value) -> LogLevel {
    let n = |k: &str| data.get(k).and_then(Value::as_u64).unwrap_or(0);
    let s = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    match kind {
        "state" if s("to") == "failed" => LogLevel::Error,
        "state" if !s("error_code").is_empty() => LogLevel::Warn,
        "finished" => match s("state") {
            "failed" => LogLevel::Error,
            "completed_with_warnings" => LogLevel::Warn,
            _ => LogLevel::Info,
        },
        "retry" | "space_low" | "entry_skipped" => LogLevel::Warn,
        "transfer_round" if n("failed") > 0 => LogLevel::Warn,
        "verify" if n("mismatch") + n("missing") + n("unverifiable") > 0 => LogLevel::Warn,
        "analysis" if n("scan_errors") > 0 => LogLevel::Warn,
        "integration"
            if data
                .get("problems")
                .and_then(Value::as_array)
                .is_some_and(|p| !p.is_empty()) =>
        {
            LogLevel::Warn
        }
        _ => LogLevel::Info,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn levels() {
        assert_eq!(
            log_level("state", &json!({ "to": "failed" })),
            LogLevel::Error
        );
        assert_eq!(
            log_level("state", &json!({ "to": "verifying" })),
            LogLevel::Info
        );
        assert_eq!(log_level("retry", &json!({})), LogLevel::Warn);
        assert_eq!(log_level("space_low", &json!({})), LogLevel::Warn);
        assert_eq!(
            log_level(
                "state",
                &json!({ "to": "paused", "error_code": "destination_full" })
            ),
            LogLevel::Warn
        );
        assert_eq!(
            log_level("transfer_round", &json!({ "failed": 0 })),
            LogLevel::Info
        );
        assert_eq!(
            log_level("transfer_round", &json!({ "failed": 2 })),
            LogLevel::Warn
        );
        assert_eq!(
            log_level("verify", &json!({ "missing": 1 })),
            LogLevel::Warn
        );
        assert_eq!(
            log_level("finished", &json!({ "state": "completed_with_warnings" })),
            LogLevel::Warn
        );
        assert_eq!(
            log_level("integration", &json!({ "problems": [] })),
            LogLevel::Info
        );
        assert_eq!(
            log_level("integration", &json!({ "problems": [{}] })),
            LogLevel::Warn
        );
        assert_eq!(log_level("something_new", &json!(null)), LogLevel::Info);
        assert_eq!(default_label(&["/home/demo/Fotos/".into()]), "Fotos");
        assert_eq!(default_label(&["/a/B".into(), "/c".into()]), "B +1");
    }
}
