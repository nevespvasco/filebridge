//! The engine's recent log lines, kept in memory for Settings → About → "Copy log" (D07): what a
//! bug report needs without asking the user to find the service's stderr or the journal.
//!
//! The lines are what `tracing` prints at info level and above, as plain text. Secrets never
//! reach `tracing` (ADR 0004: `Redacted<T>`, and rclone's lines are scrubbed before they are
//! logged); each line is passed through rclone's connection-string redaction once more before it
//! is kept, so a secret parameter that slipped into a message still leaves as `***`.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::Subscriber;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::registry::LookupSpan;

/// How many lines the process keeps: about a day of a busy service's info lines.
pub const RECENT_LINES: usize = 2000;
/// A longer line is cut here (a huge error body is not worth 2000 slots of memory).
const MAX_LINE: usize = 4096;

/// The last [`RECENT_LINES`] log lines of this process, oldest first.
#[derive(Clone)]
pub struct RecentLog {
    lines: Arc<Mutex<VecDeque<String>>>,
    cap: usize,
}

impl RecentLog {
    pub fn new(cap: usize) -> Self {
        Self {
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(cap.min(256)))),
            cap,
        }
    }

    /// The process's log, which [`Self::layer`] fills and `service_log` reads.
    pub fn global() -> &'static RecentLog {
        static GLOBAL: OnceLock<RecentLog> = OnceLock::new();
        GLOBAL.get_or_init(|| RecentLog::new(RECENT_LINES))
    }

    /// A `tracing` layer that writes info-level lines and above into this log, whatever filter
    /// the process's other layers (stderr) use.
    pub fn layer<S>(&self) -> impl Layer<S> + Send + Sync + 'static
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(self.clone())
            .with_filter(LevelFilter::INFO)
    }

    /// The lines, oldest first.
    pub fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .expect("log lock")
            .iter()
            .cloned()
            .collect()
    }

    fn push(&self, text: &str) {
        let mut lines = self.lines.lock().expect("log lock");
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let mut line = filebridge_transport_rclone::fs::redact(line);
            if line.len() > MAX_LINE {
                let mut cut = MAX_LINE;
                while !line.is_char_boundary(cut) {
                    cut -= 1;
                }
                line.truncate(cut);
                line.push('…');
            }
            if lines.len() == self.cap {
                lines.pop_front();
            }
            lines.push_back(line);
        }
    }
}

impl<'a> MakeWriter<'a> for RecentLog {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> LineWriter {
        LineWriter {
            log: self.clone(),
            buf: Vec::new(),
        }
    }
}

/// One event's text: the fmt layer writes it here, and it is kept when the writer is dropped.
pub struct LineWriter {
    log: RecentLog,
    buf: Vec<u8>,
}

impl io::Write for LineWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        if !self.buf.is_empty() {
            self.log.push(&String::from_utf8_lossy(&self.buf));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    fn with_log(log: &RecentLog, f: impl FnOnce()) {
        let subscriber = tracing_subscriber::registry().with(log.layer());
        tracing::subscriber::with_default(subscriber, f);
    }

    #[test]
    fn keeps_info_and_above_oldest_first() {
        let log = RecentLog::new(10);
        with_log(&log, || {
            tracing::debug!("not kept");
            tracing::info!(job = "j1", "started");
            tracing::warn!("slow disk");
        });
        let lines = log.lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("INFO") && lines[0].contains("started"));
        assert!(lines[0].contains("job=\"j1\""), "{}", lines[0]);
        assert!(lines[1].contains("WARN") && lines[1].contains("slow disk"));
        assert!(!lines[0].contains('\u{1b}'), "no colour codes");
    }

    #[test]
    fn keeps_only_the_last_lines() {
        let log = RecentLog::new(3);
        with_log(&log, || {
            for i in 0..5 {
                tracing::info!("line {i}");
            }
        });
        let lines = log.lines();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].ends_with("line 2") && lines[2].ends_with("line 4"));
    }

    #[test]
    fn secret_parameters_leave_redacted() {
        let log = RecentLog::new(10);
        with_log(&log, || {
            tracing::warn!("rclone: bad fs :sftp,host=example.invalid,pass=hunter2:/data");
        });
        let line = &log.lines()[0];
        assert!(!line.contains("hunter2"), "{line}");
        assert!(line.contains("pass=***"), "{line}");
    }

    #[test]
    fn long_lines_are_cut() {
        let log = RecentLog::new(10);
        with_log(&log, || tracing::info!("{}", "é".repeat(MAX_LINE)));
        let line = &log.lines()[0];
        assert!(line.len() <= MAX_LINE + '…'.len_utf8());
        assert!(line.ends_with('…'));
    }
}
