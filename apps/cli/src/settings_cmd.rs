//! `filebridge settings`: the app-wide bandwidth limit and send window (docs/adr/0010).

use anyhow::{Context, Result, bail};
use filebridge_core::{AppSettings, SendWindow};
use filebridge_host::Access;
use filebridge_ipc::Operations;

use crate::app::{App, Global};

#[derive(clap::Subcommand)]
pub enum SettingsCmd {
    /// Print the current settings.
    Show,
    /// Change some settings; the others stay as they are.
    Set {
        /// The most every send together may use, rclone-style (`20M` = 20 MiB/s; a plain
        /// number is KiB/s), or `off`.
        #[arg(long)]
        bwlimit: Option<String>,
        /// When files may be sent, local time, e.g. `22:00-07:00`, or `off` for any time.
        #[arg(long)]
        window: Option<String>,
    },
}

pub async fn run(g: &Global, cmd: Option<SettingsCmd>) -> Result<()> {
    match cmd.unwrap_or(SettingsCmd::Show) {
        SettingsCmd::Show => {
            let app = App::open(g, Access::Read, false).await?;
            print(&app.ops.get_settings().await?);
        }
        SettingsCmd::Set { bwlimit, window } => {
            let app = App::open(g, Access::Exclusive, false).await?;
            let mut settings = app.ops.get_settings().await?;
            if let Some(b) = bwlimit {
                settings.bandwidth_limit = parse_rate(&b)?;
            }
            if let Some(w) = window {
                settings.send_window = parse_window(&w)?;
            }
            app.ops.save_settings(settings.clone()).await?;
            print(&settings);
        }
    }
    Ok(())
}

fn print(s: &AppSettings) {
    let limit = match s.bandwidth_limit {
        Some(b) => format!("{}/s", crate::send_cmd::human_bytes(b)),
        None => "off".into(),
    };
    let window = match s.send_window {
        Some(w) => format!("{}-{}", hm(w.start_minute), hm(w.end_minute)),
        None => "any time".into(),
    };
    println!("bandwidth limit  {limit}");
    println!("send window      {window}");
}

fn hm(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// rclone's size form: `512` (KiB), `512K`, `20M`, `1.5G`, `800B`; or `off`.
fn parse_rate(s: &str) -> Result<Option<u64>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let (num, unit) = match s.char_indices().find(|(_, c)| c.is_ascii_alphabetic()) {
        Some((i, _)) => s.split_at(i),
        None => (s, "K"),
    };
    let factor: f64 = match unit.to_ascii_uppercase().as_str() {
        "B" => 1.0,
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        "G" => 1024.0 * 1024.0 * 1024.0,
        _ => bail!("unknown unit {unit:?} in {s:?}; use B, K, M or G"),
    };
    let n: f64 = num
        .parse()
        .with_context(|| format!("not a bandwidth limit: {s:?}"))?;
    if !n.is_finite() || n <= 0.0 {
        bail!("the bandwidth limit must be more than zero");
    }
    Ok(Some((n * factor).round() as u64))
}

/// `HH:MM-HH:MM`, or `off`.
fn parse_window(s: &str) -> Result<Option<SendWindow>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let minute = |t: &str| -> Result<u16> {
        let (h, m) = t
            .trim()
            .split_once(':')
            .with_context(|| format!("not a time: {t:?} (use HH:MM)"))?;
        let (h, m): (u16, u16) = (h.parse()?, m.parse()?);
        if h > 23 || m > 59 {
            bail!("not a time: {t:?}");
        }
        Ok(h * 60 + m)
    };
    let (start, end) = s
        .split_once('-')
        .with_context(|| format!("not a window: {s:?} (use HH:MM-HH:MM)"))?;
    Ok(Some(SendWindow {
        start_minute: minute(start)?,
        end_minute: minute(end)?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates() {
        assert_eq!(parse_rate("off").unwrap(), None);
        assert_eq!(parse_rate("512").unwrap(), Some(512 * 1024));
        assert_eq!(parse_rate("20M").unwrap(), Some(20 * 1024 * 1024));
        assert_eq!(parse_rate("1.5g").unwrap(), Some(1_610_612_736));
        assert_eq!(parse_rate("800B").unwrap(), Some(800));
        assert!(parse_rate("fast").is_err());
        assert!(parse_rate("10X").is_err());
        assert!(parse_rate("0").is_err());
    }

    #[test]
    fn windows() {
        assert_eq!(parse_window("off").unwrap(), None);
        assert_eq!(
            parse_window("22:00-07:30").unwrap(),
            Some(SendWindow {
                start_minute: 22 * 60,
                end_minute: 7 * 60 + 30
            })
        );
        assert!(parse_window("22:00").is_err());
        assert!(parse_window("24:00-07:00").is_err());
    }
}
