//! A file's Category: a coarse kind from its name's extension, used to show what a job contains
//! (docs/design/specs/D02-source-and-composition.md, "Category table"). It never changes how a
//! file is sent.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum FileCategory {
    Image,
    Video,
    Audio,
    Document,
    Archive,
    Code,
    Other,
}

/// Longer extensions are not looked up (they are names with a dot, not extensions).
const MAX_EXTENSION: usize = 10;

const TABLE: &[(FileCategory, &[&str])] = &[
    (
        FileCategory::Image,
        &[
            "jpg", "jpeg", "png", "gif", "webp", "avif", "heic", "heif", "tif", "tiff", "bmp",
            "svg", "psd", "xcf", "raw", "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2", "raf",
        ],
    ),
    (
        FileCategory::Video,
        &[
            "mp4", "m4v", "mov", "mkv", "avi", "wmv", "webm", "mts", "m2ts", "3gp", "mpg", "mpeg",
            "vob", "flv",
        ],
    ),
    (
        FileCategory::Audio,
        &[
            "mp3", "m4a", "aac", "flac", "alac", "wav", "aif", "aiff", "ogg", "oga", "opus", "wma",
            "mid", "midi",
        ],
    ),
    (
        FileCategory::Document,
        &[
            "pdf", "txt", "md", "rtf", "doc", "docx", "odt", "xls", "xlsx", "ods", "csv", "ppt",
            "pptx", "odp", "pages", "numbers", "key", "epub",
        ],
    ),
    (
        FileCategory::Archive,
        &[
            "zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "iso", "dmg",
        ],
    ),
    (
        FileCategory::Code,
        &[
            "rs", "ts", "tsx", "js", "jsx", "vue", "py", "go", "java", "kt", "c", "h", "cpp",
            "hpp", "cs", "rb", "php", "sh", "ps1", "sql", "html", "css", "scss", "json", "yaml",
            "yml", "toml", "xml", "ipynb",
        ],
    ),
];

impl FileCategory {
    /// From a file name (or a path: only the last component counts) as raw bytes: the text after
    /// the last `.`, ASCII-lowercased. No extension, a dotfile (`.bashrc`), a non-UTF-8 or overly
    /// long extension → `Other`.
    pub fn of_name(name: &[u8]) -> Self {
        let name = name.rsplit(|&b| b == b'/').next().unwrap_or(name);
        let Some(dot) = name.iter().rposition(|&b| b == b'.') else {
            return Self::Other;
        };
        if dot == 0 {
            return Self::Other;
        }
        let ext = &name[dot + 1..];
        if ext.is_empty() || ext.len() > MAX_EXTENSION || !ext.is_ascii() {
            return Self::Other;
        }
        let ext = ext.to_ascii_lowercase();
        let ext = std::str::from_utf8(&ext).unwrap_or_default();
        TABLE
            .iter()
            .find(|(_, exts)| exts.contains(&ext))
            .map_or(Self::Other, |(c, _)| *c)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Document => "document",
            Self::Archive => "archive",
            Self::Code => "code",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Image,
            Self::Video,
            Self::Audio,
            Self::Document,
            Self::Archive,
            Self::Code,
            Self::Other,
        ]
        .into_iter()
        .find(|c| c.as_str() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::FileCategory::{self, *};

    #[test]
    fn by_extension() {
        assert_eq!(
            FileCategory::of_name(b"IMG_0001.JPG"),
            Image,
            "case-insensitive"
        );
        assert_eq!(
            FileCategory::of_name(b"a.tar.gz"),
            Archive,
            "the last extension counts"
        );
        assert_eq!(
            FileCategory::of_name(b"Fotos/2019/clip.MOV"),
            Video,
            "only the name counts"
        );
        assert_eq!(FileCategory::of_name(b"Ata.pdf"), Document);
        assert_eq!(FileCategory::of_name(b"main.rs"), Code);
        assert_eq!(FileCategory::of_name(b"song.flac"), Audio);
    }

    #[test]
    fn other() {
        for name in [
            &b".bashrc"[..],
            b"README",
            b"x.\xff",
            b"name.",
            b"report.final-version-2",
            b"dir.jpg/notes",
        ] {
            assert_eq!(FileCategory::of_name(name), Other, "{name:?}");
        }
    }

    #[test]
    fn round_trips_through_text() {
        for c in [Image, Video, Audio, Document, Archive, Code, Other] {
            assert_eq!(FileCategory::parse(c.as_str()), Some(c));
        }
        assert_eq!(FileCategory::parse("nope"), None);
    }
}
