//! Disks mounted on this computer that files can be sent from (docs/design/specs/D02): external
//! and removable disks, memory cards, and disks mounted under `/media` or `/mnt`. Read-only: it
//! reads the mount table and asks for free space, and never mounts or unmounts anything.
//!
//! - Linux: `/proc/self/mountinfo`. udisks (what file managers use) mounts a disk under
//!   `/run/media/$USER` or `/media/$USER`, so it shows up there; whether the disk can be
//!   unplugged comes from sysfs. A disk plugged in but not mounted yet isn't listed.
//! - macOS: the folders in `/Volumes`, except the startup disk.
//! - Windows: the drive letters other than the system drive.
//! - Elsewhere (phones): nothing.

use std::path::{Path, PathBuf};

/// One mounted disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// Where it is mounted.
    pub path: PathBuf,
    /// The mount folder's name (udisks names it after the disk's label).
    pub label: String,
    /// The disk can be unplugged, as far as the system says.
    pub removable: bool,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
}

/// The disks mounted now, in the order the system lists them.
pub fn volumes() -> Vec<Volume> {
    listed()
        .into_iter()
        .map(|(path, removable)| {
            let (total_bytes, free_bytes) = space(&path);
            Volume {
                label: label(&path),
                path,
                removable,
                total_bytes,
                free_bytes,
            }
        })
        .collect()
}

fn label(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

#[cfg(target_os = "linux")]
fn listed() -> Vec<(PathBuf, bool)> {
    let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    linux::select(&linux::parse_mountinfo(&text), linux::removable)
}

#[cfg(target_os = "macos")]
fn listed() -> Vec<(PathBuf, bool)> {
    let Ok(dir) = std::fs::read_dir("/Volumes") else {
        return Vec::new();
    };
    // The startup disk is a link to `/` there.
    let mut out: Vec<(PathBuf, bool)> = dir
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| (e.path(), true))
        .collect();
    out.sort();
    out
}

#[cfg(windows)]
fn listed() -> Vec<(PathBuf, bool)> {
    let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    (b'A'..=b'Z')
        .map(|l| format!("{}:", l as char))
        .filter(|d| !d.eq_ignore_ascii_case(&system))
        .map(|d| PathBuf::from(format!("{d}\\")))
        .filter(|p| p.exists())
        .map(|p| (p, false))
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn listed() -> Vec<(PathBuf, bool)> {
    Vec::new()
}

/// Total and free bytes of the filesystem `path` is on.
#[cfg(unix)]
fn space(path: &Path) -> (Option<u64>, Option<u64>) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return (None, None);
    };
    // SAFETY: `c` is a valid C string and `stat` a zeroed out-parameter statvfs fills.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 {
        return (None, None);
    }
    #[allow(clippy::unnecessary_cast)] // the field types differ between systems
    let unit = if stat.f_frsize > 0 {
        stat.f_frsize as u64
    } else {
        stat.f_bsize as u64
    };
    #[allow(clippy::unnecessary_cast)]
    let (blocks, avail) = (stat.f_blocks as u64, stat.f_bavail as u64);
    (Some(blocks * unit), Some(avail * unit))
}

#[cfg(not(unix))]
fn space(_path: &Path) -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod linux {
    use std::path::{Path, PathBuf};

    /// One line of `/proc/self/mountinfo`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Mount {
        pub point: PathBuf,
        /// The device, e.g. `/dev/sdb1`.
        pub source: String,
    }

    /// Mount folders that are a user's disks whatever the device says: where udisks and people
    /// mount them.
    const MEDIA_DIRS: &[&str] = &["/run/media/", "/media/", "/mnt/"];
    /// Where udisks mounts: a disk there is one the user plugged in.
    const UDISKS_DIRS: &[&str] = &["/run/media/", "/media/"];

    pub fn parse_mountinfo(text: &str) -> Vec<Mount> {
        text.lines()
            .filter_map(|line| {
                let (left, right) = line.split_once(" - ")?;
                let point = left.split(' ').nth(4)?;
                // After the separator: the filesystem type, then the device.
                let source = unescape(right.split(' ').nth(1)?);
                Some(Mount {
                    point: PathBuf::from(unescape(point)),
                    source,
                })
            })
            .collect()
    }

    /// The kernel writes a space, tab, newline and backslash in a path as `\ooo` (octal).
    fn unescape(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            let octal = b
                .get(i + 1..i + 4)
                .is_some_and(|d| d.iter().all(|c| (b'0'..=b'7').contains(c)));
            if b[i] == b'\\' && octal {
                let n = b[i + 1..i + 4]
                    .iter()
                    .fold(0u32, |n, c| n * 8 + u32::from(c - b'0'));
                if let Ok(byte) = u8::try_from(n) {
                    out.push(byte);
                    i += 4;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into())
    }

    /// The mounts that are a user's disks, each with whether it can be unplugged: a block
    /// device (not a snap's loop device) mounted under a media folder, or anywhere but the
    /// system's own folders when `removable` says the device can be unplugged. The last mount
    /// of a folder wins, as in the kernel.
    pub fn select(mounts: &[Mount], removable: impl Fn(&str) -> bool) -> Vec<(PathBuf, bool)> {
        let mut out: Vec<(PathBuf, bool)> = Vec::new();
        for m in mounts {
            out.retain(|(p, _)| p != &m.point);
            if !m.source.starts_with("/dev/") || m.source.starts_with("/dev/loop") {
                continue;
            }
            let point = m.point.to_string_lossy();
            let unpluggable = removable(&m.source);
            let in_media = MEDIA_DIRS.iter().any(|d| point.starts_with(d));
            if !(in_media || unpluggable && !system_dir(&m.point)) {
                continue;
            }
            let by_udisks = UDISKS_DIRS.iter().any(|d| point.starts_with(d));
            out.push((m.point.clone(), unpluggable || by_udisks));
        }
        out
    }

    fn system_dir(p: &Path) -> bool {
        p == Path::new("/")
            || ["/boot", "/efi", "/usr", "/var", "/home", "/snap"]
                .iter()
                .any(|d| p.starts_with(d))
    }

    /// Whether the device can be unplugged: sysfs marks its disk removable (a memory card), or
    /// the disk hangs off a USB bus. A device-mapper device (an encrypted disk) is asked about
    /// the device below it.
    pub fn removable(source: &str) -> bool {
        let dev = std::fs::canonicalize(source).unwrap_or_else(|_| PathBuf::from(source));
        let Some(name) = dev.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            return false;
        };
        unpluggable(&name, 2)
    }

    fn unpluggable(name: &str, depth: u8) -> bool {
        let class = Path::new("/sys/class/block").join(name);
        let Ok(node) = std::fs::canonicalize(&class) else {
            return false;
        };
        if node.to_string_lossy().contains("/usb") {
            return true;
        }
        // A partition's disk is the folder above it.
        let disk = if class.join("partition").exists() {
            node.parent().map(Path::to_path_buf).unwrap_or(node)
        } else {
            node
        };
        if std::fs::read_to_string(disk.join("removable")).is_ok_and(|s| s.trim() == "1") {
            return true;
        }
        depth > 0
            && std::fs::read_dir(class.join("slaves")).is_ok_and(|d| {
                d.flatten()
                    .any(|e| unpluggable(&e.file_name().to_string_lossy(), depth - 1))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::linux::{Mount, parse_mountinfo, select};
    use std::path::PathBuf;

    const MOUNTINFO: &str = "\
22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
23 22 0:21 / /proc rw,nosuid shared:5 - proc proc rw
24 22 259:1 / /boot/efi rw,relatime shared:2 - vfat /dev/nvme0n1p1 rw
25 22 7:3 / /snap/core/123 ro,nodev shared:9 - squashfs /dev/loop3 ro
26 22 0:44 / /run/user/1000 rw shared:300 - tmpfs tmpfs rw
27 22 8:17 / /run/media/demo/Disco\\040Externo rw,nosuid shared:400 - exfat /dev/sdb1 rw
28 22 8:33 / /mnt/backup rw shared:401 - ext4 /dev/sdc1 rw
29 22 0:50 / /mnt/nas rw shared:402 - cifs //nas.example/share rw
30 22 8:49 / /srv/usb rw shared:403 - vfat /dev/sdd1 rw
31 22 8:65 / /srv/internal rw shared:404 - ext4 /dev/sde1 rw
32 22 8:66 / /home rw shared:405 - ext4 /dev/sdf1 rw
";

    #[test]
    fn mountinfo_lines_are_read_with_their_escapes() {
        let mounts = parse_mountinfo(MOUNTINFO);
        assert_eq!(mounts.len(), 11);
        assert_eq!(
            mounts[5],
            Mount {
                point: PathBuf::from("/run/media/demo/Disco Externo"),
                source: "/dev/sdb1".into(),
            }
        );
        assert!(parse_mountinfo("garbage\n\n").is_empty());
    }

    #[test]
    fn only_a_users_disks_are_offered() {
        let mounts = parse_mountinfo(MOUNTINFO);
        // sdd and sdf are USB disks; nothing else can be unplugged.
        let got = select(&mounts, |dev| dev == "/dev/sdd1" || dev == "/dev/sdf1");
        assert_eq!(
            got,
            [
                (PathBuf::from("/run/media/demo/Disco Externo"), true),
                (PathBuf::from("/mnt/backup"), false),
                (PathBuf::from("/srv/usb"), true),
            ],
            "not the system's disks, a snap, a network share, or /home"
        );
    }

    #[test]
    fn the_last_mount_of_a_folder_wins() {
        let text = "\
40 1 8:17 / /media/demo/CARD rw - vfat /dev/sdb1 rw
41 1 0:60 / /media/demo/CARD rw - fuse.sshfs host:/x rw
";
        assert!(select(&parse_mountinfo(text), |_| true).is_empty());
    }

    #[test]
    fn this_computer_answers_without_failing() {
        // Whatever is mounted here: each one has a path and a name.
        for v in super::volumes() {
            assert!(v.path.is_absolute(), "{v:?}");
            assert!(!v.label.is_empty(), "{v:?}");
        }
    }
}
