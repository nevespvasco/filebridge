//! What file system a local destination folder is on, and so which name rules it needs: an
//! exFAT disk takes Windows names whatever machine it is plugged into. The UI shows what was
//! detected instead of asking (docs/design/specs/D03). Reads metadata only, writes nothing.

use std::path::{Path, PathBuf};

use filebridge_model::{Family, LocalTargetProbe};

use crate::{CoreError, Engine, Result};

impl Engine {
    /// The file system of `path`, or of its nearest existing parent (the folder may not exist
    /// yet), on this machine.
    pub async fn probe_local_target(&self, path: String) -> Result<LocalTargetProbe> {
        tokio::task::spawn_blocking(move || probe(Path::new(&path)))
            .await
            .map_err(|e| CoreError::Invalid(format!("file system check failed: {e}")))
    }
}

/// See [`Engine::probe_local_target`]. Everything `None` when nothing could be told.
pub(crate) fn probe(path: &Path) -> LocalTargetProbe {
    let Some(dir) = nearest_existing(path) else {
        return LocalTargetProbe::default();
    };
    let (fs_type, case_sensitive) = os::fs_type(&dir);
    let rules = fs_type.as_deref().and_then(rules_of);
    LocalTargetProbe {
        family: rules.map(|(f, _)| f).or_else(os::family),
        // What the file system itself says wins over what its type usually means.
        case_sensitive: case_sensitive
            .or_else(|| rules.map(|(_, cs)| cs))
            .or_else(os::case_sensitive),
        fs_type,
    }
}

/// `path` made absolute, symbolic links resolved, or its nearest parent that exists.
fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut p = std::path::absolute(path).ok()?;
    loop {
        if let Ok(real) = std::fs::canonicalize(&p) {
            return Some(real);
        }
        if !p.pop() {
            return None;
        }
    }
}

/// The name rules (family, case sensitivity) a file system type usually needs; `None` for one
/// filebridge doesn't know. `fs_type` as macOS's `statfs` or Linux's mount table names it.
pub(crate) fn rules_of(fs_type: &str) -> Option<(Family, bool)> {
    let t = fs_type.to_ascii_lowercase();
    let t = t.strip_prefix("fuse.").unwrap_or(&t);
    match t {
        // fuseblk: a FUSE driver for a block device, in practice ntfs-3g or exfat-fuse.
        "exfat" | "vfat" | "msdos" | "fat" | "fat12" | "fat16" | "fat32" | "ntfs" | "ntfs3"
        | "ntfs-3g" | "fuseblk" => Some((Family::Windows, false)),
        "apfs" | "hfs" | "hfsplus" => Some((Family::Macos, false)),
        "ext2" | "ext3" | "ext4" | "btrfs" | "xfs" | "zfs" | "f2fs" | "tmpfs" => {
            Some((Family::Unix, true))
        }
        _ => None,
    }
}

/// The file system type of the mount `path` is on, from `/proc/self/mountinfo`'s text: the
/// longest mount point that contains it (the last one, when mounted over).
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) fn mount_fs_type(mountinfo: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for line in mountinfo.lines() {
        // id parent major:minor root mount-point options [optional…] - type source super-options
        let mut fields = line.split(' ');
        let Some(mount_point) = fields.nth(4) else {
            continue;
        };
        let Some(fs_type) = fields.skip_while(|f| *f != "-").nth(1) else {
            continue;
        };
        let mount_point = unescape(mount_point);
        let mp = Path::new(&mount_point);
        if !path.starts_with(mp) {
            continue;
        }
        let depth = mp.components().count();
        if best.is_none_or(|(d, _)| depth >= d) {
            best = Some((depth, fs_type));
        }
    }
    best.map(|(_, t)| t.to_owned())
}

/// Undoes mountinfo's octal escapes (`\040` for a space).
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && (b'0'..=b'3').contains(&b[i + 1])
            && b[i + 2..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_vendor = "apple")]
mod os {
    use std::ffi::{CStr, CString};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    use filebridge_model::Family;

    /// `<sys/unistd.h>`: whether a volume tells upper and lower case apart.
    const PC_CASE_SENSITIVE: libc::c_int = 11;

    /// `statfs`'s type name, and `pathconf`'s case sensitivity.
    pub(super) fn fs_type(dir: &Path) -> (Option<String>, Option<bool>) {
        let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else {
            return (None, None);
        };
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `c` is a valid C string; `st` is a properly sized, writable statfs.
        let fs_type = (unsafe { libc::statfs(c.as_ptr(), &mut st) } == 0).then(|| {
            // SAFETY: the kernel NUL-terminates f_fstypename within its 16 bytes.
            unsafe { CStr::from_ptr(st.f_fstypename.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        });
        // SAFETY: `c` is a valid C string.
        let case_sensitive = match unsafe { libc::pathconf(c.as_ptr(), PC_CASE_SENSITIVE) } {
            1 => Some(true),
            0 => Some(false),
            _ => None,
        };
        (fs_type, case_sensitive)
    }

    pub(super) fn family() -> Option<Family> {
        None
    }

    pub(super) fn case_sensitive() -> Option<bool> {
        None
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod os {
    use std::path::Path;

    use filebridge_model::Family;

    /// The type of the mount the folder is on; Linux has no portable case-sensitivity query.
    pub(super) fn fs_type(dir: &Path) -> (Option<String>, Option<bool>) {
        let fs_type = std::fs::read_to_string("/proc/self/mountinfo")
            .ok()
            .and_then(|m| super::mount_fs_type(&m, dir));
        (fs_type, None)
    }

    pub(super) fn family() -> Option<Family> {
        None
    }

    pub(super) fn case_sensitive() -> Option<bool> {
        None
    }
}

/// Windows: every local volume takes Windows names, NTFS and exFAT alike, without telling case
/// apart by default.
#[cfg(windows)]
mod os {
    use std::path::Path;

    use filebridge_model::Family;

    pub(super) fn fs_type(_dir: &Path) -> (Option<String>, Option<bool>) {
        (None, None)
    }

    pub(super) fn family() -> Option<Family> {
        Some(Family::Windows)
    }

    pub(super) fn case_sensitive() -> Option<bool> {
        Some(false)
    }
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    windows
)))]
mod os {
    use std::path::Path;

    use filebridge_model::Family;

    pub(super) fn fs_type(_dir: &Path) -> (Option<String>, Option<bool>) {
        (None, None)
    }

    pub(super) fn family() -> Option<Family> {
        None
    }

    pub(super) fn case_sensitive() -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_systems_map_to_name_rules() {
        for t in [
            "exfat", "vfat", "msdos", "fat32", "ntfs", "ntfs3", "fuseblk", "NTFS",
        ] {
            assert_eq!(rules_of(t), Some((Family::Windows, false)), "{t}");
        }
        assert_eq!(rules_of("fuse.ntfs-3g"), Some((Family::Windows, false)));
        for t in ["apfs", "hfs"] {
            assert_eq!(rules_of(t), Some((Family::Macos, false)), "{t}");
        }
        for t in [
            "ext2", "ext3", "ext4", "btrfs", "xfs", "zfs", "f2fs", "tmpfs",
        ] {
            assert_eq!(rules_of(t), Some((Family::Unix, true)), "{t}");
        }
        for t in ["nfs", "smbfs", "fuse", "overlay", ""] {
            assert_eq!(rules_of(t), None, "{t}: unknown");
        }
    }

    #[test]
    fn the_longest_mount_point_wins() {
        let mountinfo = "\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
40 22 8:17 / /media/usb rw,nosuid - exfat /dev/sdb1 rw,fmask=0022
41 22 8:33 / /media/usb\\040disk rw - fuseblk /dev/sdc1 rw,user_id=0
42 22 0:40 / /media/usbx rw - vfat /dev/sdd1 rw
43 40 0:41 / /media/usb rw - ntfs3 /dev/sde1 rw
";
        let t = |p: &str| mount_fs_type(mountinfo, Path::new(p));
        assert_eq!(t("/home/u/Backup").as_deref(), Some("ext4"));
        assert_eq!(
            t("/media/usb/Fotos").as_deref(),
            Some("ntfs3"),
            "mounted over: the last one"
        );
        assert_eq!(t("/media/usb disk/a").as_deref(), Some("fuseblk"));
        assert_eq!(
            t("/media/usbx").as_deref(),
            Some("vfat"),
            "whole components only"
        );
        assert_eq!(mount_fs_type("garbage\n", Path::new("/")), None);
    }

    #[test]
    fn a_folder_not_made_yet_is_probed_at_its_parent() {
        let dir = tempfile::tempdir().unwrap();
        let p = probe(&dir.path().join("not/there/yet"));
        let here = probe(dir.path());
        assert_eq!(p, here);
    }
}
