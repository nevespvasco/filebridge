//! App-wide settings the service keeps (docs/adr/0010): one bandwidth limit for every send, and
//! the daily window when files may be sent.

use serde::{Deserialize, Serialize};

const DAY_SECS: u32 = 24 * 60 * 60;

/// The smallest bandwidth limit accepted, in bytes per second.
pub const MIN_BANDWIDTH_LIMIT: u64 = 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct AppSettings {
    /// Bytes per second across all sends at once; `None` = unlimited.
    pub bandwidth_limit: Option<u64>,
    /// When files may be sent, in the service machine's local time; `None` = any time.
    pub send_window: Option<SendWindow>,
}

impl AppSettings {
    /// Why these settings can't be saved, if they can't.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(limit) = self.bandwidth_limit
            && limit < MIN_BANDWIDTH_LIMIT
        {
            return Err(format!(
                "the bandwidth limit must be at least {MIN_BANDWIDTH_LIMIT} bytes per second"
            ));
        }
        if let Some(w) = &self.send_window {
            w.validate()?;
        }
        Ok(())
    }
}

/// A daily window, `start` included and `end` excluded. `start > end` crosses midnight
/// (22:00–07:00).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SendWindow {
    /// Minutes since local midnight, 0–1439.
    pub start_minute: u16,
    /// Minutes since local midnight, 0–1439.
    pub end_minute: u16,
}

/// Whether a window is open at some moment, and how long until that changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowStatus {
    pub open: bool,
    /// Seconds until the window opens (when closed) or closes (when open).
    pub change_in_secs: u32,
}

impl SendWindow {
    pub fn validate(&self) -> Result<(), String> {
        if self.start_minute >= 24 * 60 || self.end_minute >= 24 * 60 {
            return Err("a send window time must be between 00:00 and 23:59".into());
        }
        if self.start_minute == self.end_minute {
            return Err("a send window must start and end at different times".into());
        }
        Ok(())
    }

    /// The window at `second_of_day` (0–86399, local wall-clock time). Callers re-check at
    /// least every minute, so a clock change or a DST jump is picked up within a minute.
    pub fn status(&self, second_of_day: u32) -> WindowStatus {
        let now = second_of_day % DAY_SECS;
        let start = u32::from(self.start_minute) * 60;
        let end = u32::from(self.end_minute) * 60;
        let open = if start < end {
            (start..end).contains(&now)
        } else {
            now >= start || now < end
        };
        let next = if open { end } else { start };
        WindowStatus {
            open,
            change_in_secs: (next + DAY_SECS - now - 1) % DAY_SECS + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(start: &str, end: &str) -> SendWindow {
        let m = |s: &str| {
            let (h, m) = s.split_once(':').unwrap();
            h.parse::<u16>().unwrap() * 60 + m.parse::<u16>().unwrap()
        };
        SendWindow {
            start_minute: m(start),
            end_minute: m(end),
        }
    }

    fn at(hm: &str) -> u32 {
        let (h, m) = hm.split_once(':').unwrap();
        h.parse::<u32>().unwrap() * 3600 + m.parse::<u32>().unwrap() * 60
    }

    #[test]
    fn same_day_window() {
        let win = w("09:00", "17:00");
        assert_eq!(
            win.status(at("08:00")),
            WindowStatus {
                open: false,
                change_in_secs: 3600
            }
        );
        assert_eq!(
            win.status(at("09:00")),
            WindowStatus {
                open: true,
                change_in_secs: 8 * 3600
            },
            "start is included"
        );
        assert_eq!(
            win.status(at("17:00")),
            WindowStatus {
                open: false,
                change_in_secs: 16 * 3600
            },
            "end is excluded"
        );
        assert_eq!(win.status(at("16:59") + 59).change_in_secs, 1);
    }

    #[test]
    fn window_across_midnight() {
        let win = w("22:00", "07:00");
        assert!(win.status(at("23:30")).open);
        assert!(win.status(at("00:00")).open);
        assert_eq!(win.status(at("06:00")).change_in_secs, 3600);
        assert_eq!(
            win.status(at("07:00")),
            WindowStatus {
                open: false,
                change_in_secs: 15 * 3600
            }
        );
        assert_eq!(win.status(at("21:00")).change_in_secs, 3600);
    }

    #[test]
    fn whole_day_but_one_minute() {
        let win = w("00:01", "00:00");
        assert!(!win.status(at("00:00")).open);
        assert_eq!(win.status(at("00:00")).change_in_secs, 60);
        assert!(win.status(at("12:00")).open);
        assert_eq!(win.status(at("23:59")).change_in_secs, 60);
    }

    #[test]
    fn validation() {
        assert!(AppSettings::default().validate().is_ok());
        assert!(w("22:00", "22:00").validate().is_err());
        let late = SendWindow {
            start_minute: 1440,
            end_minute: 0,
        };
        assert!(late.validate().is_err());
        let slow = AppSettings {
            bandwidth_limit: Some(10),
            send_window: None,
        };
        assert!(slow.validate().is_err());
        let ok = AppSettings {
            bandwidth_limit: Some(2_500_000),
            send_window: Some(w("22:00", "07:00")),
        };
        assert!(ok.validate().is_ok());
    }
}
