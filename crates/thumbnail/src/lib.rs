//! Small previews of source images (docs/design/specs/D05, ADR 0027): made from the local source,
//! which is only read, and kept in a cache folder of bounded size, least recently used first out.
//! Nothing is written next to the source.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, Limits};
use sha2::{Digest, Sha256};

/// The longest side of a thumbnail, in pixels.
pub const SIZE: u32 = 256;
/// The cache's default size on disk.
pub const DEFAULT_MAX_BYTES: u64 = 128 * 1024 * 1024;
/// Bumped when the output changes (size, format, quality), so older cache entries stop matching.
const VERSION: u8 = 1;
/// Sources larger than this get no preview.
const MAX_SOURCE_BYTES: u64 = 256 * 1024 * 1024;
/// The most memory one decode may take (a 50-megapixel photo needs about 150 MB).
const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;
const JPEG_QUALITY: u8 = 80;
/// What an entry costs on disk: its length rounded up to a file system block, so the markers of
/// files without a preview count too.
const BLOCK: u64 = 4096;
/// Eviction goes down to this share of the maximum, so it doesn't run on every insert.
const EVICT_TO_PERCENT: u64 = 90;
/// A temporary file older than this was left by a crash.
const STALE_TEMP: Duration = Duration::from_secs(3600);

/// The extensions a preview is tried for. The content decides the format; this only spares
/// opening files that are certainly not images.
const EXTENSIONS: &[&str] = &[
    "bmp", "gif", "jfif", "jpe", "jpeg", "jpg", "png", "tif", "tiff", "webp",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// `image/jpeg`, or `image/png` when the image has transparency.
    pub mime: &'static str,
    pub bytes: Vec<u8>,
}

/// Whether a preview is tried for `path`, by its extension. No I/O.
pub fn can_preview(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// A thumbnail of the image at `source`, which is only read. `Ok(None)`: not an image this crate
/// decodes, damaged, or too large. `Err`: the source couldn't be read.
pub fn make(source: &Path) -> io::Result<Option<Thumbnail>> {
    let file = File::open(source)?;
    if file.metadata()?.len() > MAX_SOURCE_BYTES {
        return Ok(None);
    }
    let mut reader = ImageReader::new(BufReader::new(file)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    let decoded = reader.into_decoder().and_then(|mut decoder| {
        let orientation = decoder.orientation()?;
        let mut img = DynamicImage::from_decoder(decoder)?;
        img.apply_orientation(orientation);
        Ok(img)
    });
    let img = match decoded {
        Ok(img) => img,
        // A truncated or garbled file reads fine and decodes to nothing.
        Err(ImageError::IoError(e))
            if !matches!(
                e.kind(),
                io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData
            ) =>
        {
            return Err(e);
        }
        Err(e) => {
            tracing::debug!("no preview for {}: {e}", source.display());
            return Ok(None);
        }
    };
    encode(img).map(Some)
}

fn encode(img: DynamicImage) -> io::Result<Thumbnail> {
    let img = if img.width() > SIZE || img.height() > SIZE {
        img.thumbnail(SIZE, SIZE)
    } else {
        img
    };
    let mut bytes = Vec::new();
    if img.color().has_alpha() {
        let rgba = img.to_rgba8();
        if rgba.pixels().any(|p| p[3] < u8::MAX) {
            DynamicImage::ImageRgba8(rgba)
                .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
                .map_err(io::Error::other)?;
            return Ok(Thumbnail {
                mime: "image/png",
                bytes,
            });
        }
    }
    JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
        .encode_image(&img.to_rgb8())
        .map_err(io::Error::other)?;
    Ok(Thumbnail {
        mime: "image/jpeg",
        bytes,
    })
}

/// What one cache file holds, by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Jpeg,
    Png,
    /// The source has no preview: remembered so it isn't decoded again.
    None,
}

impl Kind {
    const ALL: [Kind; 3] = [Kind::Jpeg, Kind::Png, Kind::None];

    fn ext(self) -> &'static str {
        match self {
            Kind::Jpeg => "jpg",
            Kind::Png => "png",
            Kind::None => "none",
        }
    }

    fn mime(self) -> Option<&'static str> {
        match self {
            Kind::Jpeg => Some("image/jpeg"),
            Kind::Png => Some("image/png"),
            Kind::None => None,
        }
    }

    fn of(thumb: Option<&Thumbnail>) -> Kind {
        match thumb.map(|t| t.mime) {
            Some("image/png") => Kind::Png,
            Some(_) => Kind::Jpeg,
            None => Kind::None,
        }
    }
}

/// A source as the cache knows it: its path, size and modification time. A changed file gets a
/// new key, and its old thumbnail ages out.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key(String);

impl Key {
    fn of(source: &Path, meta: &fs::Metadata) -> Key {
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .unwrap_or_default();
        let path = source.as_os_str().as_encoded_bytes();
        let mut h = Sha256::new();
        h.update([VERSION]);
        h.update(SIZE.to_le_bytes());
        h.update((path.len() as u64).to_le_bytes());
        h.update(path);
        h.update(meta.len().to_le_bytes());
        h.update(mtime.as_secs().to_le_bytes());
        h.update(mtime.subsec_nanos().to_le_bytes());
        let digest = h.finalize();
        Key(digest[..16].iter().map(|b| format!("{b:02x}")).collect())
    }
}

/// What the cache knows about a source.
#[derive(Debug)]
pub enum Lookup {
    /// Made before: the thumbnail, or `None` when the source has no preview.
    Found(Option<Thumbnail>),
    /// Not made yet: [`Thumbnails::fill`] makes it.
    Missing(Key),
}

#[derive(Debug)]
struct Slot {
    kind: Kind,
    cost: u64,
    /// When it was last used: larger is more recent.
    used: u64,
}

#[derive(Debug, Default)]
struct Index {
    slots: HashMap<String, Slot>,
    total: u64,
    clock: u64,
}

impl Index {
    fn tick(&mut self) -> u64 {
        self.clock = (self.clock + 1).max(now_nanos());
        self.clock
    }
}

/// The thumbnail cache: one folder (the app's cache, never next to a source), at most
/// `max_bytes` on disk. Safe to share between threads; blocking, so async callers run it on a
/// blocking thread.
#[derive(Debug)]
pub struct Thumbnails {
    dir: PathBuf,
    max_bytes: u64,
    /// Read from the folder on first use.
    index: Mutex<Option<Index>>,
    temp: AtomicU64,
}

impl Thumbnails {
    /// A cache in `dir`, created on first write.
    pub fn new(dir: PathBuf, max_bytes: u64) -> Self {
        Self {
            dir,
            max_bytes,
            index: Mutex::new(None),
            temp: AtomicU64::new(0),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Bytes on disk, as counted for the limit.
    pub fn size(&self) -> u64 {
        self.with_index(|i| i.total)
    }

    /// The thumbnail of `source`, from the cache or made now.
    pub fn get(&self, source: &Path) -> io::Result<Option<Thumbnail>> {
        match self.lookup(source)? {
            Lookup::Found(t) => Ok(t),
            Lookup::Missing(key) => self.fill(source, &key),
        }
    }

    /// What the cache has for `source`, without decoding anything. A source whose extension is
    /// not an image's, or that is not a file, is `Found(None)`.
    pub fn lookup(&self, source: &Path) -> io::Result<Lookup> {
        let meta = fs::metadata(source)?;
        if !meta.is_file() || !can_preview(source) {
            return Ok(Lookup::Found(None));
        }
        let key = Key::of(source, &meta);
        let Some(kind) = self.with_index(|i| i.slots.get(&key.0).map(|s| s.kind)) else {
            return Ok(Lookup::Missing(key));
        };
        let path = self.path(&key.0, kind);
        let found = match kind.mime() {
            None => Ok(None),
            Some(mime) => fs::read(&path).map(|bytes| Some(Thumbnail { mime, bytes })),
        };
        match found {
            Ok(thumb) => {
                self.with_index(|i| {
                    let used = i.tick();
                    if let Some(s) = i.slots.get_mut(&key.0) {
                        s.used = used;
                    }
                });
                // So the order survives a restart. Best effort.
                let _ = File::options()
                    .write(true)
                    .open(&path)
                    .and_then(|f| f.set_modified(SystemTime::now()));
                Ok(Lookup::Found(thumb))
            }
            // Removed behind the cache's back: make it again.
            Err(_) => {
                self.with_index(|i| {
                    if let Some(s) = i.slots.remove(&key.0) {
                        i.total -= s.cost;
                    }
                });
                Ok(Lookup::Missing(key))
            }
        }
    }

    /// Makes the thumbnail [`lookup`](Self::lookup) found missing and keeps it (or that there is
    /// none). A source that couldn't be read is not remembered: the next call tries again.
    pub fn fill(&self, source: &Path, key: &Key) -> io::Result<Option<Thumbnail>> {
        let thumb = make(source)?;
        if let Err(e) = self.store(key, thumb.as_ref()) {
            tracing::warn!("could not cache a thumbnail in {}: {e}", self.dir.display());
        }
        Ok(thumb)
    }

    fn store(&self, key: &Key, thumb: Option<&Thumbnail>) -> io::Result<()> {
        create_private_dir(&self.dir)?;
        let kind = Kind::of(thumb);
        let bytes = thumb.map_or(&[][..], |t| &t.bytes);
        let n = self.temp.fetch_add(1, Ordering::Relaxed);
        let temp = self
            .dir
            .join(format!(".{}.{}-{n}.tmp", key.0, std::process::id()));
        fs::write(&temp, bytes)?;
        let path = self.path(&key.0, kind);
        if let Err(e) = fs::rename(&temp, &path) {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
        let cost = cost(bytes.len() as u64);
        let evicted = self.with_index(|i| {
            let used = i.tick();
            if let Some(old) = i.slots.insert(key.0.clone(), Slot { kind, cost, used }) {
                i.total -= old.cost;
                if old.kind != kind {
                    let _ = fs::remove_file(self.path(&key.0, old.kind));
                }
            }
            i.total += cost;
            self.evict(i)
        });
        for (name, kind) in evicted {
            let _ = fs::remove_file(self.path(&name, kind));
        }
        Ok(())
    }

    /// Drops the least recently used entries from the index until it fits; answers the files to
    /// delete.
    fn evict(&self, i: &mut Index) -> Vec<(String, Kind)> {
        if i.total <= self.max_bytes {
            return Vec::new();
        }
        let target = self.max_bytes / 100 * EVICT_TO_PERCENT;
        let mut by_age: Vec<(u64, String)> =
            i.slots.iter().map(|(k, s)| (s.used, k.clone())).collect();
        by_age.sort_unstable();
        let mut out = Vec::new();
        for (_, name) in by_age {
            if i.total <= target {
                break;
            }
            if let Some(s) = i.slots.remove(&name) {
                i.total -= s.cost;
                out.push((name, s.kind));
            }
        }
        out
    }

    fn path(&self, key: &str, kind: Kind) -> PathBuf {
        self.dir.join(format!("{key}.{}", kind.ext()))
    }

    fn with_index<T>(&self, f: impl FnOnce(&mut Index) -> T) -> T {
        let mut guard = self.index.lock().unwrap_or_else(|e| e.into_inner());
        let index = guard.get_or_insert_with(|| load(&self.dir));
        f(index)
    }
}

/// The cache folder as it is on disk. Files it didn't write are left alone.
fn load(dir: &Path) -> Index {
    let mut index = Index::default();
    let Ok(read) = fs::read_dir(dir) else {
        return index;
    };
    for entry in read.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let modified = meta.modified().ok();
        if name.starts_with('.') && name.ends_with(".tmp") {
            let stale = modified
                .and_then(|m| m.elapsed().ok())
                .is_some_and(|age| age > STALE_TEMP);
            if stale {
                let _ = fs::remove_file(entry.path());
            }
            continue;
        }
        let Some((stem, ext)) = name.split_once('.') else {
            continue;
        };
        let Some(kind) = Kind::ALL.into_iter().find(|k| k.ext() == ext) else {
            continue;
        };
        if stem.len() != 32 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) || !meta.is_file() {
            continue;
        }
        let used = modified
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        let cost = cost(meta.len());
        index.total += cost;
        index.clock = index.clock.max(used);
        index
            .slots
            .insert(stem.to_owned(), Slot { kind, cost, used });
    }
    index
}

fn cost(len: u64) -> u64 {
    len.div_ceil(BLOCK).max(1) * BLOCK
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// The cache holds pictures of the user's files: only the user may read it.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut b = fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

#[cfg(test)]
mod tests;
