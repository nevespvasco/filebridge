//! Parsers for the text the server prints. Pure functions; the fixtures in `tests/fixtures/` are
//! what real Proxmox and Nextcloud output looks like.

use serde::{Deserialize, Serialize};

/// One row of `pct list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LxcContainer {
    pub ctid: u32,
    pub status: String,
    pub name: String,
}

impl LxcContainer {
    pub fn running(&self) -> bool {
        self.status == "running"
    }
}

/// `pct list`: `VMID Status Lock Name`, where Lock is often blank.
pub fn pct_list(out: &str) -> Vec<LxcContainer> {
    out.lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            let ctid = cols.first()?.parse().ok()?;
            Some(LxcContainer {
                ctid,
                status: cols.get(1)?.to_string(),
                name: if cols.len() >= 3 {
                    cols[cols.len() - 1].to_string()
                } else {
                    String::new()
                },
            })
        })
        .collect()
}

/// Where an `mpN` mount comes from on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MountSource {
    /// Bind mount of a host directory.
    HostPath(String),
    /// Proxmox storage volume (`local-zfs:subvol-105-disk-1`); `pvesm path` resolves it.
    Volume(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// `mp0`, `mp1`, …
    pub key: String,
    pub source: MountSource,
    /// Path inside the container.
    pub target: String,
    pub read_only: bool,
}

/// One `lxc.idmap` range: container ids `[container, container+count)` map to `host…`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdMap {
    /// `'u'` or `'g'`.
    pub kind: char,
    pub container: u32,
    pub host: u32,
    pub count: u32,
}

/// The parts of `pct config <id>` direct mode needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LxcConfig {
    pub hostname: Option<String>,
    pub unprivileged: bool,
    pub mounts: Vec<Mount>,
    pub idmap: Vec<IdMap>,
}

/// Proxmox's default for unprivileged containers: `u/g 0 100000 65536`.
pub const DEFAULT_ID_OFFSET: u32 = 100_000;
const DEFAULT_ID_COUNT: u32 = 65_536;

impl LxcConfig {
    /// Host id for container id `id`. With no `lxc.idmap` lines: identity when privileged,
    /// `100000 + id` when unprivileged. With idmap lines, only they count (as in Proxmox).
    pub fn map_id(&self, kind: char, id: u32) -> Option<u32> {
        if self.idmap.is_empty() {
            return if !self.unprivileged {
                Some(id)
            } else if id < DEFAULT_ID_COUNT {
                Some(DEFAULT_ID_OFFSET + id)
            } else {
                None
            };
        }
        self.idmap
            .iter()
            .filter(|m| m.kind == kind)
            .find(|m| id >= m.container && id - m.container < m.count)
            .map(|m| m.host + (id - m.container))
    }

    /// The mount whose container path holds `path` (longest match), and `path` relative to it.
    pub fn mount_for<'a>(&'a self, path: &str) -> Option<(&'a Mount, String)> {
        self.mounts
            .iter()
            .filter_map(|m| {
                let target = m.target.trim_end_matches('/');
                let rest = path.strip_prefix(target)?;
                (rest.is_empty() || rest.starts_with('/'))
                    .then(|| (m, rest.trim_start_matches('/').to_string()))
            })
            .max_by_key(|(m, _)| m.target.len())
    }
}

/// `pct config <id>`. Stops at the first snapshot section (`[name]`).
pub fn pct_config(out: &str) -> LxcConfig {
    let mut cfg = LxcConfig::default();
    for line in out.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            break;
        }
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':').or_else(|| line.split_once('=')) else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        match key {
            "hostname" => cfg.hostname = Some(value.to_string()),
            "unprivileged" => cfg.unprivileged = value == "1",
            "lxc.idmap" => {
                let parts: Vec<&str> = value.split_whitespace().collect();
                if let [kind @ ("u" | "g"), c, h, n] = parts[..]
                    && let (Ok(container), Ok(host), Ok(count)) = (c.parse(), h.parse(), n.parse())
                {
                    cfg.idmap.push(IdMap {
                        kind: kind.chars().next().unwrap(),
                        container,
                        host,
                        count,
                    });
                }
            }
            k if k.starts_with("mp") && k[2..].parse::<u32>().is_ok() => {
                if let Some(m) = parse_mount(k, value) {
                    cfg.mounts.push(m);
                }
            }
            _ => {}
        }
    }
    cfg
}

/// `mp0: /tank/data,mp=/mnt/data,backup=0` or `mp1: volume=local-zfs:subvol-1,mp=/srv,ro=1`.
fn parse_mount(key: &str, value: &str) -> Option<Mount> {
    let mut source = None;
    let mut target = None;
    let mut read_only = false;
    for (i, part) in value.split(',').enumerate() {
        match part.split_once('=') {
            Some(("volume", v)) => source = Some(v),
            Some(("mp", v)) => target = Some(v),
            Some(("ro", v)) => read_only = v == "1",
            Some(_) => {}
            None if i == 0 => source = Some(part),
            None => {}
        }
    }
    let source = source?;
    Some(Mount {
        key: key.to_string(),
        source: if source.starts_with('/') {
            MountSource::HostPath(source.to_string())
        } else {
            MountSource::Volume(source.to_string())
        },
        target: target?.to_string(),
        read_only,
    })
}

/// The `'datadirectory' => '…'` value from `config.php` (or from `grep datadirectory` of it).
/// Commented lines are skipped.
pub fn datadirectory(config_php: &str) -> Option<String> {
    config_php.lines().find_map(|line| {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('#') || t.starts_with('*') {
            return None;
        }
        let rest = t
            .strip_prefix("'datadirectory'")
            .or_else(|| t.strip_prefix("\"datadirectory\""))?;
        let rest = rest.trim_start().strip_prefix("=>")?.trim_start();
        php_string(rest)
    })
}

/// A PHP string literal at the start of `s` (single or double quoted, basic escapes).
fn php_string(s: &str) -> Option<String> {
    let mut chars = s.chars();
    let q = chars.next().filter(|c| *c == '\'' || *c == '"')?;
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                e if e == q || e == '\\' => out.push(e),
                e => {
                    out.push('\\');
                    out.push(e);
                }
            },
            c if c == q => return Some(out),
            c => out.push(c),
        }
    }
    None
}

/// `id <user>`: `uid=33(www-data) gid=33(www-data) groups=33(www-data)`.
pub fn id_output(out: &str) -> Option<(u32, u32)> {
    let field = |name: &str| {
        out.split_whitespace()
            .find_map(|w| w.strip_prefix(name))
            .and_then(|v| v.split('(').next())
            .and_then(|n| n.parse().ok())
    };
    Some((field("uid=")?, field("gid=")?))
}

/// `stat -c %u:%g <path>`.
pub fn uid_gid(out: &str) -> Option<(u32, u32)> {
    let (u, g) = out.trim().split_once(':')?;
    Some((u.parse().ok()?, g.parse().ok()?))
}

/// Available bytes from `df -Pk <path>` (the field before the `NN%` capacity column).
pub fn df_available(out: &str) -> Option<u64> {
    let line = out.lines().nth(1)?;
    let cols: Vec<&str> = line.split_whitespace().collect();
    let cap = cols.iter().position(|c| c.ends_with('%'))?;
    let kb: u64 = cols.get(cap.checked_sub(1)?)?.parse().ok()?;
    kb.checked_mul(1024)
}

/// `occ status --output=json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OccStatus {
    pub installed: bool,
    #[serde(default)]
    pub versionstring: String,
    #[serde(default)]
    pub maintenance: bool,
    #[serde(default)]
    pub needs_db_upgrade: bool,
}

pub fn occ_status(out: &str) -> Option<OccStatus> {
    // PHP warnings can precede the JSON; take the last line that parses.
    out.lines()
        .rev()
        .find_map(|l| serde_json::from_str(l.trim()).ok())
}

/// One classified line of `occ files:scan -v` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanLine {
    File(String),
    Folder(String),
    /// `Entry "<path>" will not be accessible due to incompatible encoding`.
    Encoding(String),
    /// Retryable when another scan holds the lock.
    Error {
        message: String,
        retryable: bool,
    },
    Other,
}

const ERROR_MARKERS: &[&str] = &[
    "Error while scanning",
    "Home storage for user",
    "Path not found",
    "Exception during scan",
    "Error during scan",
    "Unknown user",
    "Make sure you're running the scan command only as the user",
];

pub fn scan_line(line: &str) -> ScanLine {
    let t = strip_tags(line.trim());
    if let Some(p) = t.strip_prefix("File\t") {
        return ScanLine::File(p.trim().to_string());
    }
    if let Some(p) = t.strip_prefix("Folder\t") {
        return ScanLine::Folder(p.trim().to_string());
    }
    if let Some(rest) = t.strip_prefix("Entry \"")
        && let Some(path) =
            rest.strip_suffix("\" will not be accessible due to incompatible encoding")
    {
        return ScanLine::Encoding(path.to_string());
    }
    if t.starts_with("Another process is already scanning") {
        return ScanLine::Error {
            message: t,
            retryable: true,
        };
    }
    if ERROR_MARKERS.iter().any(|m| t.starts_with(m)) {
        return ScanLine::Error {
            message: t,
            retryable: false,
        };
    }
    ScanLine::Other
}

/// Removes Symfony console tags (`<info>`, `<error>`) in case output was decorated.
fn strip_tags(s: &str) -> String {
    let mut out = s.to_string();
    for tag in [
        "<info>",
        "</info>",
        "<error>",
        "</error>",
        "<comment>",
        "</comment>",
    ] {
        out = out.replace(tag, "");
    }
    out
}

/// Totals from the summary table `occ files:scan` prints at the end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanTotals {
    pub folders: u64,
    pub files: u64,
    pub errors: u64,
}

pub fn scan_totals(out: &str) -> Option<ScanTotals> {
    let rows: Vec<Vec<String>> = out
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('|'))
        .map(|l| {
            l.trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect();
    let header = rows
        .iter()
        .position(|r| r.first().is_some_and(|c| c == "Folders"))?;
    let (names, values) = (&rows[header], rows.get(header + 1)?);
    let get = |name: &str| -> Option<u64> {
        let i = names.iter().position(|n| n == name)?;
        values.get(i)?.parse().ok()
    };
    Some(ScanTotals {
        folders: get("Folders")?,
        files: get("Files")?,
        errors: get("Errors").unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PCT_LIST: &str = include_str!("../tests/fixtures/pct_list.txt");
    const PCT_CONFIG: &str = include_str!("../tests/fixtures/pct_config_105.txt");
    const PCT_CONFIG_IDMAP: &str = include_str!("../tests/fixtures/pct_config_custom_idmap.txt");
    const CONFIG_PHP: &str = include_str!("../tests/fixtures/config.php");
    const SCAN: &str = include_str!("../tests/fixtures/occ_files_scan_verbose.txt");

    #[test]
    fn parses_pct_list() {
        let cts = pct_list(PCT_LIST);
        assert_eq!(cts.len(), 4);
        assert_eq!(
            cts[1],
            LxcContainer {
                ctid: 105,
                status: "running".into(),
                name: "nextcloud".into()
            }
        );
        assert_eq!(cts[2].name, "backup-ct", "lock column present");
        assert!(!cts[3].running());
    }

    #[test]
    fn parses_pct_config_mounts_and_default_idmap() {
        let cfg = pct_config(PCT_CONFIG);
        assert_eq!(cfg.hostname.as_deref(), Some("nextcloud"));
        assert!(cfg.unprivileged);
        assert!(cfg.idmap.is_empty());
        assert_eq!(cfg.mounts.len(), 2);
        assert_eq!(
            cfg.mounts[0].source,
            MountSource::HostPath("/tank/nextcloud-data".into())
        );
        assert_eq!(cfg.mounts[0].target, "/mnt/ncdata");
        assert_eq!(
            cfg.mounts[1].source,
            MountSource::Volume("local-zfs:subvol-105-disk-1".into())
        );
        assert!(cfg.mounts[1].read_only);
        assert_eq!(cfg.map_id('u', 33), Some(100_033));
        assert_eq!(cfg.map_id('g', 33), Some(100_033));
        let (m, rest) = cfg.mount_for("/mnt/ncdata/data").unwrap();
        assert_eq!((m.key.as_str(), rest.as_str()), ("mp0", "data"));
        assert!(cfg.mount_for("/mnt/ncdatax").is_none());
        assert!(cfg.mount_for("/var/www/nextcloud/data").is_none());
    }

    #[test]
    fn custom_idmap_wins_and_snapshots_are_ignored() {
        let cfg = pct_config(PCT_CONFIG_IDMAP);
        assert_eq!(cfg.idmap.len(), 6);
        // www-data (33) passed through to host uid 1005, gid 1005.
        assert_eq!(cfg.map_id('u', 33), Some(1005));
        assert_eq!(cfg.map_id('g', 33), Some(1005));
        assert_eq!(cfg.map_id('u', 0), Some(100_000));
        assert_eq!(cfg.map_id('u', 34), Some(100_034));
        assert_eq!(cfg.map_id('u', 70_000), None);
        // The snapshot section's different mount is not read.
        assert_eq!(cfg.mounts.len(), 1);
        assert_eq!(cfg.mounts[0].target, "/srv/nextcloud/data");
    }

    #[test]
    fn privileged_is_identity() {
        let cfg = pct_config("hostname: nc\nmp0: /data,mp=/data\n");
        assert!(!cfg.unprivileged);
        assert_eq!(cfg.map_id('u', 33), Some(33));
    }

    #[test]
    fn parses_datadirectory() {
        assert_eq!(datadirectory(CONFIG_PHP).as_deref(), Some("/mnt/ncdata"));
        let grep =
            "  // 'datadirectory' => '/old',\n  \"datadirectory\" => \"/srv/O\\\"Brien data\",\n";
        assert_eq!(datadirectory(grep).as_deref(), Some("/srv/O\"Brien data"));
        assert_eq!(
            datadirectory("'datadirectory' => '/a\\'b',").as_deref(),
            Some("/a'b")
        );
        assert_eq!(datadirectory("'datadirectory' => ").as_deref(), None);
    }

    #[test]
    fn parses_id_stat_df() {
        assert_eq!(
            id_output("uid=33(www-data) gid=33(www-data) groups=33(www-data)\n"),
            Some((33, 33))
        );
        assert_eq!(id_output("id: 'www-data': no such user"), None);
        assert_eq!(uid_gid("100033:100033\n"), Some((100_033, 100_033)));
        let df = "Filesystem           1024-blocks       Used  Available Capacity Mounted on\n\
                  tank/nextcloud-data   3904897024 1175269376 2729627648      31% /tank/nextcloud-data\n";
        assert_eq!(df_available(df), Some(2_729_627_648 * 1024));
        assert_eq!(df_available("garbage"), None);
    }

    #[test]
    fn parses_occ_status() {
        let out = "PHP Warning:  something\n{\"installed\":true,\"version\":\"29.0.4.1\",\"versionstring\":\"29.0.4\",\"edition\":\"\",\"maintenance\":false,\"needsDbUpgrade\":false,\"productname\":\"Nextcloud\",\"extendedSupport\":false}\n";
        let s = occ_status(out).unwrap();
        assert!(s.installed && !s.maintenance && !s.needs_db_upgrade);
        assert_eq!(s.versionstring, "29.0.4");
        assert!(occ_status("Nextcloud is not installed").is_none());
    }

    #[test]
    fn classifies_scan_output() {
        let lines: Vec<ScanLine> = SCAN.lines().map(scan_line).collect();
        let files = lines
            .iter()
            .filter(|l| matches!(l, ScanLine::File(_)))
            .count();
        let folders = lines
            .iter()
            .filter(|l| matches!(l, ScanLine::Folder(_)))
            .count();
        assert_eq!((files, folders), (4, 3));
        let enc: Vec<_> = lines
            .iter()
            .filter_map(|l| match l {
                ScanLine::Encoding(p) => Some(p.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(enc, ["/alice/files/Fotos/Vera\u{303}o"]);
        assert_eq!(
            scan_totals(SCAN),
            Some(ScanTotals {
                folders: 3,
                files: 4,
                errors: 1
            })
        );
        assert!(matches!(
            scan_line("Another process is already scanning '/alice/files/Fotos'"),
            ScanLine::Error {
                retryable: true,
                ..
            }
        ));
        assert!(matches!(
            scan_line("Path not found: /alice/files/Nope"),
            ScanLine::Error {
                retryable: false,
                ..
            }
        ));
    }

    #[test]
    fn old_scan_table_without_errors_column() {
        let out = "+---------+-------+--------------+\n| Folders | Files | Elapsed time |\n+---------+-------+--------------+\n| 2       | 7     | 00:00:00     |\n+---------+-------+--------------+\n";
        assert_eq!(
            scan_totals(out),
            Some(ScanTotals {
                folders: 2,
                files: 7,
                errors: 0
            })
        );
    }
}
