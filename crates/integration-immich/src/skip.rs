//! Which sent files a library scan passes over, so the preflight and the report can say so
//! before anyone looks for them in Immich. Immich's scan globs each import path for
//! `**/*{<the extensions it imports>}` with hidden names off and the library's exclusion
//! patterns ignored, matching case-insensitively (fast-glob); this mirrors that.

use filebridge_integration_api::{Problem, RemotePath};

use crate::api::{Library, MediaTypes};
use crate::settings::covers;

/// Problem codes (also the UI's i18n keys under `integration.`).
pub const EXCLUDED: &str = "immich.excluded";
pub const UNSUPPORTED: &str = "immich.unsupported";
pub const HIDDEN: &str = "immich.hidden";

/// What Immich imports, when the server doesn't say (`/server/media-types` is missing on older
/// versions): its own lists, from `server/src/utils/mime-types.ts`.
const IMAGE: &[&str] = &[
    "3fr", "ari", "arw", "avif", "bmp", "cap", "cin", "cr2", "cr3", "crw", "dcr", "dng", "erf",
    "fff", "gif", "heic", "heif", "hif", "iiq", "insp", "jfif", "jp2", "jpe", "jpeg", "jpg", "jxl",
    "k25", "kdc", "mpo", "mrw", "nef", "nrw", "orf", "ori", "pef", "png", "psd", "raf", "raw",
    "rw2", "rwl", "sr2", "srf", "srw", "svg", "tif", "tiff", "webp", "x3f",
];
const VIDEO: &[&str] = &[
    "3gp", "3gpp", "avi", "flv", "insv", "m2t", "m2ts", "m4v", "mkv", "mov", "mp4", "mpe", "mpeg",
    "mpg", "mts", "mxf", "ts", "vob", "webm", "wmv",
];
/// Read beside a photo, never imported on its own: not worth a warning.
const SIDECAR: &[&str] = &["xmp"];

/// The library's rules for one destination folder.
#[derive(Debug, Clone)]
pub struct Skips {
    /// The destination's root as Immich sees it.
    import_path: String,
    /// The library's import path that holds it: names below it are what the scan looks at.
    import_root: String,
    patterns: Vec<(String, Vec<String>)>,
    /// Lower case, without the dot.
    extensions: Vec<String>,
}

impl Skips {
    /// `media` `None`: Immich's own lists ([`IMAGE`], [`VIDEO`]).
    pub fn new(library: &Library, import_path: &str, media: Option<&MediaTypes>) -> Self {
        let import_root = library
            .import_paths
            .iter()
            .find(|p| covers(p, import_path))
            .map(|p| p.trim_end_matches('/').to_owned())
            .unwrap_or_else(|| import_path.to_owned());
        let extensions = match media {
            Some(m) if !(m.image.is_empty() && m.video.is_empty()) => m
                .image
                .iter()
                .chain(&m.video)
                .chain(&m.sidecar)
                .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
                .collect(),
            _ => IMAGE
                .iter()
                .chain(VIDEO)
                .chain(SIDECAR)
                .map(|e| (*e).to_owned())
                .collect(),
        };
        let patterns = library
            .exclusion_patterns
            .iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| (p.clone(), expand_braces(&p.to_lowercase())))
            .collect();
        Self {
            import_path: import_path.trim_end_matches('/').to_owned(),
            import_root,
            patterns,
            extensions,
        }
    }

    /// Why the scan passes over `file` (destination-relative), if it does.
    pub fn check(&self, file: &RemotePath) -> Option<Problem> {
        let full = format!("{}/{}", self.import_path, file.as_str());
        let problem = |code: &str, message: String| Problem {
            code: code.into(),
            message,
            path: Some(file.to_string()),
            retryable: false,
            fix: None,
        };
        let lower = full.to_lowercase();
        // fast-glob drops a folder its ignore patterns match, and everything inside it.
        let ancestors: Vec<&str> = lower
            .match_indices('/')
            .map(|(i, _)| &lower[..i])
            .filter(|a| !a.is_empty())
            .collect();
        for (pattern, alternatives) in &self.patterns {
            let hit = |p: &&str| alternatives.iter().any(|a| glob(a, p));
            if hit(&lower.as_str()) || ancestors.iter().any(hit) {
                return Some(problem(
                    EXCLUDED,
                    format!("matches the library's exclusion pattern {pattern}"),
                ));
            }
        }
        let below = full
            .strip_prefix(&self.import_root)
            .unwrap_or(file.as_str())
            .trim_start_matches('/');
        if below.split('/').any(|part| part.starts_with('.')) {
            return Some(problem(
                HIDDEN,
                "a hidden name (starting with “.”), which Immich's scan skips".into(),
            ));
        }
        let ext = file
            .file_name()
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase());
        match ext {
            Some(e) if self.extensions.contains(&e) => None,
            Some(e) => Some(problem(
                UNSUPPORTED,
                format!("Immich doesn't import .{e} files"),
            )),
            None => Some(problem(
                UNSUPPORTED,
                "no extension, so Immich doesn't import it".into(),
            )),
        }
    }

    pub fn check_all(&self, files: &[RemotePath]) -> Vec<Problem> {
        files.iter().filter_map(|f| self.check(f)).collect()
    }
}

/// `{a,b}` alternatives, nested ones too, as separate patterns.
fn expand_braces(pattern: &str) -> Vec<String> {
    let bytes = pattern.as_bytes();
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_owned()];
    };
    let (mut depth, mut close, mut commas) = (0, None, Vec::new());
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            b',' if depth == 1 => commas.push(i),
            _ => {}
        }
    }
    let Some(close) = close else {
        return vec![pattern.to_owned()];
    };
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    let mut bounds = vec![open];
    bounds.extend(&commas);
    bounds.push(close);
    let mut out = Vec::new();
    for w in bounds.windows(2) {
        let middle = &pattern[w[0] + 1..w[1]];
        out.extend(expand_braces(&format!("{head}{middle}{tail}")));
    }
    out
}

/// A glob over a `/`-separated path: `**` is any number of whole folders, `*` anything within
/// one name, `?` one character, `[…]` a class (`!` or `^` negates).
fn glob(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let parts: Vec<&str> = path.split('/').collect();
    segments(&pat, &parts)
}

fn segments(pat: &[&str], parts: &[&str]) -> bool {
    match pat.split_first() {
        None => parts.is_empty(),
        Some((&"**", rest)) => (0..=parts.len()).any(|skip| segments(rest, &parts[skip..])),
        Some((p, rest)) => match parts.split_first() {
            Some((part, others)) => name(p.as_bytes(), part.as_bytes()) && segments(rest, others),
            None => false,
        },
    }
}

fn name(p: &[u8], s: &[u8]) -> bool {
    match p.first() {
        None => s.is_empty(),
        Some(b'*') => (0..=s.len()).any(|i| name(&p[1..], &s[i..])),
        Some(b'?') => !s.is_empty() && name(&p[1..], &s[1..]),
        Some(b'[') => match (class(&p[1..], s.first().copied()), s.split_first()) {
            (Some((true, used)), Some((_, rest))) => name(&p[1 + used..], rest),
            (None, Some((c, rest))) => *c == b'[' && name(&p[1..], rest),
            _ => false,
        },
        Some(b'\\') if p.len() > 1 => s.first() == Some(&p[1]) && name(&p[2..], &s[1..]),
        Some(c) => s.first() == Some(c) && name(&p[1..], &s[1..]),
    }
}

/// Matches `c` against a class starting after its `[`: `(matched, bytes up to and with ']')`,
/// or `None` when the class isn't closed (then `[` is a plain character).
fn class(p: &[u8], c: Option<u8>) -> Option<(bool, usize)> {
    let c = c?;
    let negated = matches!(p.first(), Some(b'!' | b'^'));
    let mut i = usize::from(negated);
    let mut hit = false;
    let mut first = true;
    while i < p.len() {
        if p[i] == b']' && !first {
            return Some((hit != negated, i + 1));
        }
        first = false;
        if i + 2 < p.len() && p[i + 1] == b'-' && p[i + 2] != b']' {
            hit |= p[i] <= c && c <= p[i + 2];
            i += 3;
        } else {
            hit |= p[i] == c;
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(patterns: &[&str]) -> Library {
        Library {
            id: "6f1c2a4e-0b7d-4c55-9a8e-3d2f1b0c9e77".into(),
            name: "Fotos".into(),
            import_paths: vec!["/mnt/photos".into()],
            exclusion_patterns: patterns.iter().map(|p| (*p).to_owned()).collect(),
            asset_count: 0,
        }
    }

    fn code(skips: &Skips, file: &str) -> Option<String> {
        skips.check(&RemotePath::new(file).unwrap()).map(|p| p.code)
    }

    #[test]
    fn globs_match_like_fast_glob() {
        assert!(glob("**/@eadir/**", "/mnt/photos/a/@eadir/x.jpg"));
        assert!(glob("**/@eadir/**", "/mnt/photos/@eadir"));
        assert!(!glob("**/@eadir/**", "/mnt/photos/eadir/x.jpg"));
        assert!(glob("**/._*", "/mnt/photos/._img.jpg"));
        assert!(glob("/mnt/photos/raw/*.cr?", "/mnt/photos/raw/a.cr2"));
        assert!(!glob("/mnt/photos/raw/*.cr?", "/mnt/photos/raw/sub/a.cr2"));
        assert!(glob("**/img_[0-9]*.jpg", "/x/img_1.jpg"));
        assert!(!glob("**/img_[!0-9]*.jpg", "/x/img_1.jpg"));
        assert!(glob("**/[ab].jpg", "/x/b.jpg"));
        assert_eq!(
            expand_braces("**/{#recycle,#snapshot}/**"),
            ["**/#recycle/**", "**/#snapshot/**"]
        );
        assert_eq!(expand_braces("a{b,{c,d}}e"), ["abe", "ace", "ade"]);
        assert_eq!(expand_braces("a{b"), ["a{b"]);
    }

    #[test]
    fn the_scan_skips_excluded_hidden_and_unknown_files() {
        let skips = Skips::new(
            &library(&["**/@eaDir/**", "**/{#recycle,#snapshot}/**", "**/*.MOV"]),
            "/mnt/photos/Holidays",
            None,
        );
        assert_eq!(code(&skips, "2024/IMG_1.JPG"), None, "case-insensitive");
        assert_eq!(code(&skips, "2024/IMG_1.xmp"), None, "sidecars are read");
        assert_eq!(code(&skips, "2024/clip.mp4"), None);
        assert_eq!(code(&skips, "@eaDir/a.jpg").as_deref(), Some(EXCLUDED));
        assert_eq!(code(&skips, "#Recycle/a.jpg").as_deref(), Some(EXCLUDED));
        assert_eq!(code(&skips, "2024/clip.mov").as_deref(), Some(EXCLUDED));
        assert_eq!(code(&skips, ".trash/a.jpg").as_deref(), Some(HIDDEN));
        assert_eq!(code(&skips, "2024/.a.jpg").as_deref(), Some(HIDDEN));
        assert_eq!(code(&skips, "notes.txt").as_deref(), Some(UNSUPPORTED));
        assert_eq!(code(&skips, "README").as_deref(), Some(UNSUPPORTED));
        let p = skips.check(&RemotePath::new("notes.txt").unwrap()).unwrap();
        assert_eq!(p.path.as_deref(), Some("notes.txt"));
        assert!(p.message.contains(".txt"), "{}", p.message);
    }

    #[test]
    fn a_hidden_folder_above_the_destination_counts_too() {
        let skips = Skips::new(&library(&[]), "/mnt/photos/.staging", None);
        assert_eq!(code(&skips, "a.jpg").as_deref(), Some(HIDDEN));
        // The import path itself may be hidden: the scan starts there.
        let mut lib = library(&[]);
        lib.import_paths = vec!["/mnt/.photos".into()];
        let skips = Skips::new(&lib, "/mnt/.photos", None);
        assert_eq!(code(&skips, "a.jpg"), None);
    }

    #[test]
    fn the_server_s_media_types_win() {
        let media = MediaTypes {
            image: vec![".jpg".into()],
            video: vec![],
            sidecar: vec![".xmp".into()],
        };
        let skips = Skips::new(&library(&[]), "/mnt/photos", Some(&media));
        assert_eq!(code(&skips, "a.jpg"), None);
        assert_eq!(code(&skips, "a.png").as_deref(), Some(UNSUPPORTED));
    }
}
