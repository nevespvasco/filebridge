//! macOS LaunchAgent: `~/Library/LaunchAgents/app.filebridge.service.plist`, which launchd loads
//! at each login and which runs this binary like the Linux unit does: started at load, started
//! again only when it fails, its log in `~/Library/Logs/filebridge-service.log`
//! ([`crate::login`]).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use filebridge_ipc::{LoginEntry, UnitAgent, UserUnit};

use crate::login::Options;

/// The job's launchd label, after the app's identifier (`app.filebridge.desktop`).
pub const LABEL: &str = "app.filebridge.service";

/// The plist for `exe`, passing on explicit `--data-dir` / `--rclone`, and the SSH agent's
/// socket when it is pinned (see [`decide_agent`]).
pub fn render(
    exe: &Path,
    data_dir: Option<&Path>,
    rclone: Option<&Path>,
    agent: Option<&str>,
    log: &Path,
) -> String {
    let mut args = vec![exe.to_string_lossy().into_owned()];
    if let Some(d) = data_dir {
        args.push("--data-dir".into());
        args.push(d.to_string_lossy().into_owned());
    }
    if let Some(r) = rclone {
        args.push("--rclone".into());
        args.push(r.to_string_lossy().into_owned());
    }
    args.push("run".into());
    let args: String = args
        .iter()
        .map(|a| format!("\t\t<string>{}</string>\n", xml(a)))
        .collect();
    let env = agent
        .map(|sock| {
            format!(
                "\t<key>EnvironmentVariables</key>\n\
                 \t<dict>\n\
                 \t\t<key>SSH_AUTH_SOCK</key>\n\
                 \t\t<string>{}</string>\n\
                 \t</dict>\n",
                xml(sock)
            )
        })
        .unwrap_or_default();
    let log = xml(&log.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{LABEL}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         {args}\
         \t</array>\n\
         {env}\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<dict>\n\
         \t\t<key>SuccessfulExit</key>\n\
         \t\t<false/>\n\
         \t</dict>\n\
         \t<key>ThrottleInterval</key>\n\
         \t<integer>5</integer>\n\
         \t<key>StandardOutPath</key>\n\
         \t<string>{log}</string>\n\
         \t<key>StandardErrorPath</key>\n\
         \t<string>{log}</string>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// For a plist `<string>`.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// How the engine launchd starts finds the SSH agent. launchd gives every job its own
/// environment, which has macOS's agent (`/private/tmp/com.apple.launchd.…/Listeners`, moved
/// at each boot) but not one a shell profile sets, such as 1Password's or a Homebrew
/// `ssh-agent`. A socket under home keeps its path, so it is written into the plist.
pub fn decide_agent(
    sock: Option<&Path>,
    home: Option<&Path>,
    launchd_has_it: bool,
) -> (UnitAgent, Option<String>) {
    let Some(sock) = sock else {
        return (UnitAgent::None, None);
    };
    let under_home = home
        .and_then(|h| sock.strip_prefix(h).ok())
        .is_some_and(|rel| {
            rel.components().next().is_some()
                && rel
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
        });
    if under_home && let Some(s) = sock.to_str() {
        return (UnitAgent::Pinned, Some(s.to_owned()));
    }
    let reach = if launchd_has_it {
        UnitAgent::Manager
    } else {
        UnitAgent::Missing
    };
    (reach, None)
}

/// `launchctl print-disabled gui/<uid>` lists `"app.filebridge.service" => disabled` (older
/// macOS: `=> true`) once someone ran `launchctl disable` on it.
pub fn disabled_in(print_disabled: &str) -> bool {
    let quoted = format!("\"{LABEL}\"");
    print_disabled.lines().any(|l| {
        let l = l.trim();
        l.strip_prefix(&quoted)
            .map(str::trim_start)
            .and_then(|rest| rest.strip_prefix("=>"))
            .is_some_and(|v| matches!(v.trim(), "disabled" | "true"))
    })
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn plist_path() -> Result<PathBuf> {
    Ok(home()?
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

fn log_path() -> Result<PathBuf> {
    Ok(home()?.join("Library/Logs/filebridge-service.log"))
}

/// The user's launchd domain, `gui/501`.
fn domain() -> Result<String> {
    let out = Command::new("id").arg("-u").output().context("run id -u")?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !out.status.success() || uid.is_empty() {
        bail!("id -u failed");
    }
    Ok(format!("gui/{uid}"))
}

/// `launchctl <args>`: its stdout, or `None` when it fails.
fn launchctl(args: &[&str]) -> Option<String> {
    let out = Command::new("launchctl")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn launchctl_must(args: &[&str]) -> Result<()> {
    if launchctl(args).is_none() {
        bail!("launchctl {} failed", args.join(" "));
    }
    Ok(())
}

fn current_agent() -> (UnitAgent, Option<String>) {
    let sock = std::env::var_os("SSH_AUTH_SOCK")
        .map(PathBuf::from)
        // A variable left over from a dead agent is no agent.
        .filter(|p| p.exists());
    let launchd_has_it =
        launchctl(&["getenv", "SSH_AUTH_SOCK"]).is_some_and(|v| !v.trim().is_empty());
    decide_agent(
        sock.as_deref(),
        std::env::var_os("HOME").map(PathBuf::from).as_deref(),
        launchd_has_it,
    )
}

pub fn show(exe: &Path, o: &Options) -> Result<UserUnit> {
    let path = plist_path()?;
    let (reach, pin) = current_agent();
    let contents = render(exe, o.data_dir, o.rclone, pin.as_deref(), &log_path()?);
    let on_disk = std::fs::read_to_string(&path).ok();
    let domain = domain()?;
    // Answers only inside a desktop session (not over plain SSH).
    let manager = launchctl(&["print", &domain]).is_some();
    let disabled = launchctl(&["print-disabled", &domain]).is_some_and(|out| disabled_in(&out));
    Ok(UserUnit {
        kind: LoginEntry::LaunchAgent,
        path: path.display().to_string(),
        exec: exe.display().to_string(),
        installed: on_disk.is_some(),
        enabled: on_disk.is_some() && manager && !disabled,
        current: on_disk.as_deref() == Some(contents.as_str()),
        manager,
        contents,
        agent: reach,
    })
}

/// Writes the plist and clears a `launchctl disable`. With `start`, loads it now (replacing a
/// loaded older one, which stops the engine it runs); without, launchd loads it at the next
/// login.
pub fn install(exe: &Path, o: &Options, start: bool) -> Result<UnitAgent> {
    let path = plist_path()?;
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    let (reach, pin) = current_agent();
    std::fs::write(
        &path,
        render(exe, o.data_dir, o.rclone, pin.as_deref(), &log_path()?),
    )
    .with_context(|| format!("write {}", path.display()))?;
    let domain = domain()?;
    let job = format!("{domain}/{LABEL}");
    launchctl_must(&["enable", &job])?;
    if start {
        // Fails when it isn't loaded, which is fine.
        let _ = launchctl(&["bootout", &job]);
        launchctl_must(&["bootstrap", &domain, &path.to_string_lossy()])?;
    }
    println!("installed {}", path.display());
    Ok(reach)
}

pub fn uninstall() -> Result<()> {
    let path = plist_path()?;
    if !path.exists() {
        println!("not installed ({} does not exist)", path.display());
        return Ok(());
    }
    // Stops the engine it runs (SIGTERM: running jobs stay stored and resume on the next start).
    // Fails when it isn't loaded, which is fine.
    let _ = launchctl(&["bootout", &format!("{}/{LABEL}", domain()?)]);
    std::fs::remove_file(&path)?;
    println!("removed {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "/Users/demo/Library/Logs/filebridge-service.log";

    #[test]
    fn the_plist_runs_this_binary_at_login_and_again_only_when_it_fails() {
        let plist = render(
            Path::new("/Applications/filebridge.app/Contents/MacOS/filebridge-service"),
            None,
            None,
            None,
            Path::new(LOG),
        );
        assert_eq!(
            plist,
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
\t<key>Label</key>
\t<string>app.filebridge.service</string>
\t<key>ProgramArguments</key>
\t<array>
\t\t<string>/Applications/filebridge.app/Contents/MacOS/filebridge-service</string>
\t\t<string>run</string>
\t</array>
\t<key>RunAtLoad</key>
\t<true/>
\t<key>KeepAlive</key>
\t<dict>
\t\t<key>SuccessfulExit</key>
\t\t<false/>
\t</dict>
\t<key>ThrottleInterval</key>
\t<integer>5</integer>
\t<key>StandardOutPath</key>
\t<string>/Users/demo/Library/Logs/filebridge-service.log</string>
\t<key>StandardErrorPath</key>
\t<string>/Users/demo/Library/Logs/filebridge-service.log</string>
</dict>
</plist>
"
        );
    }

    #[test]
    fn options_and_the_agent_are_passed_on_escaped() {
        let plist = render(
            Path::new("/Users/demo/Apps/R&D <x>/filebridge-service"),
            Some(Path::new("/Volumes/Data/filebridge")),
            Some(Path::new("/opt/homebrew/bin/rclone")),
            Some("/Users/demo/.1password/agent.sock"),
            Path::new(LOG),
        );
        assert!(plist.contains(
            "\t<array>\n\
             \t\t<string>/Users/demo/Apps/R&amp;D &lt;x&gt;/filebridge-service</string>\n\
             \t\t<string>--data-dir</string>\n\
             \t\t<string>/Volumes/Data/filebridge</string>\n\
             \t\t<string>--rclone</string>\n\
             \t\t<string>/opt/homebrew/bin/rclone</string>\n\
             \t\t<string>run</string>\n\
             \t</array>\n\
             \t<key>EnvironmentVariables</key>\n\
             \t<dict>\n\
             \t\t<key>SSH_AUTH_SOCK</key>\n\
             \t\t<string>/Users/demo/.1password/agent.sock</string>\n\
             \t</dict>\n\
             \t<key>RunAtLoad</key>\n"
        ));
    }

    #[test]
    fn an_agent_under_home_is_pinned_and_launchds_own_is_inherited() {
        let home = Some(Path::new("/Users/demo"));
        let decide = |sock: &str, launchd: bool| decide_agent(Some(Path::new(sock)), home, launchd);
        assert_eq!(
            decide("/Users/demo/.1password/agent.sock", true),
            (
                UnitAgent::Pinned,
                Some("/Users/demo/.1password/agent.sock".into())
            )
        );
        let apple = "/private/tmp/com.apple.launchd.AbCdEf/Listeners";
        assert_eq!(decide(apple, true), (UnitAgent::Manager, None));
        assert_eq!(decide(apple, false), (UnitAgent::Missing, None));
        // `..` could leave home: not pinned.
        assert_eq!(
            decide("/Users/demo/../other/agent.sock", false),
            (UnitAgent::Missing, None)
        );
        assert_eq!(decide_agent(None, home, true), (UnitAgent::None, None));
    }

    #[test]
    fn a_launchctl_disable_is_seen_in_both_spellings() {
        let new = "disabled services = {\n\t\"com.apple.Siri\" => enabled\n\t\"app.filebridge.service\" => disabled\n}\n";
        let old = "disabled services = {\n\t\"app.filebridge.service\" => true\n}\n";
        let enabled = "disabled services = {\n\t\"app.filebridge.service\" => enabled\n\t\"app.filebridge.service.old\" => disabled\n}\n";
        assert!(disabled_in(new));
        assert!(disabled_in(old));
        assert!(!disabled_in(enabled));
        assert!(!disabled_in(""));
    }
}
