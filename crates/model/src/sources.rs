//! Quick ways to a source in step 1 (docs/design/specs/D02): pinned folders, the disks mounted
//! on this computer, and how a chosen folder's size splits between what it holds.

use serde::{Deserialize, Serialize};

/// A folder or file the user pinned, to add it again in one click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct PinnedSource {
    /// Absolute path as it was pinned.
    pub path: String,
    /// Unix ms.
    pub pinned_at: i64,
    /// Whether it is there now (a disk may be unplugged, a folder renamed).
    pub exists: bool,
}

/// The most sources that can be pinned.
pub const MAX_PINNED_SOURCES: u32 = 100;

/// A disk (or partition) mounted on the service's computer that files can be sent from: an
/// external or removable disk, a memory card, or a disk mounted under `/media` or `/mnt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct VolumeView {
    /// Where it is mounted: what is added as a source, or where the folder picker opens.
    pub path: String,
    /// Its name as the system shows it (the mount folder's name, usually the disk's label).
    pub label: String,
    /// The disk can be unplugged (USB, a memory card), as far as the system says.
    pub removable: bool,
    /// Bytes, when the system reports them.
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
}

/// What one item directly inside the measured folder holds: a folder with everything below it,
/// or a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FolderSize {
    /// Its name, lossily decoded for display.
    pub name: String,
    /// Absolute path, to measure it in turn; `None` when the name isn't valid text.
    pub path: Option<String>,
    pub is_dir: bool,
    /// Files inside (1 for a file).
    pub files: u64,
    pub bytes: u64,
}

/// A folder's size, split by what is directly inside it, most bytes first
/// (docs/design/specs/D02). Symbolic links are not followed or counted, as in the analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct FolderSizes {
    /// The measured folder, as given.
    pub path: String,
    /// At most [`FOLDER_SIZES_LISTED`], most bytes first.
    pub items: Vec<FolderSize>,
    /// Items left out of `items`: how many, and their files and bytes.
    pub rest_items: u64,
    pub rest_files: u64,
    pub rest_bytes: u64,
    /// The whole folder so far.
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    /// Entries that couldn't be read (permissions, a disk unplugged); not counted.
    pub errors: u64,
    /// Every entry was counted. `false` while it runs, and when it was cancelled.
    pub complete: bool,
}

/// The most items a [`FolderSizes`] lists one by one.
pub const FOLDER_SIZES_LISTED: usize = 200;
