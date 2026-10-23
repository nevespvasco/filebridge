use super::*;

use image::{GenericImageView, Rgb, RgbImage, Rgba, RgbaImage};

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let img = RgbImage::from_fn(w, h, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&img)
        .unwrap();
    out
}

fn png(img: RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(img)
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .unwrap();
    out
}

/// A JPEG whose EXIF says "rotate 90° clockwise to show" (orientation 6).
fn jpeg_rotated(w: u32, h: u32) -> Vec<u8> {
    let plain = jpeg(w, h);
    let mut tiff = b"Exif\0\0II*\0".to_vec();
    tiff.extend(8u32.to_le_bytes()); // first IFD
    tiff.extend(1u16.to_le_bytes()); // one entry
    tiff.extend(0x0112u16.to_le_bytes()); // Orientation
    tiff.extend(3u16.to_le_bytes()); // SHORT
    tiff.extend(1u32.to_le_bytes());
    tiff.extend(6u16.to_le_bytes());
    tiff.extend(0u16.to_le_bytes());
    tiff.extend(0u32.to_le_bytes()); // no next IFD
    let mut out = plain[..2].to_vec(); // SOI
    out.extend([0xFF, 0xE1]);
    out.extend(((tiff.len() + 2) as u16).to_be_bytes());
    out.extend(tiff);
    out.extend(&plain[2..]);
    out
}

fn dims(t: &Thumbnail) -> (u32, u32) {
    image::load_from_memory(&t.bytes).unwrap().dimensions()
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    p
}

fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}

#[test]
fn shrinks_to_fit_and_keeps_the_aspect() {
    let src = tempfile::tempdir().unwrap();
    let p = write(src.path(), "IMG_2041.JPG", &jpeg(1000, 500));
    let t = make(&p).unwrap().unwrap();
    assert_eq!(t.mime, "image/jpeg");
    assert_eq!(dims(&t), (256, 128));
}

#[test]
fn a_small_image_is_not_enlarged() {
    let src = tempfile::tempdir().unwrap();
    let p = write(src.path(), "icon.jpg", &jpeg(40, 20));
    assert_eq!(dims(&make(&p).unwrap().unwrap()), (40, 20));
}

#[test]
fn the_exif_orientation_is_applied() {
    let src = tempfile::tempdir().unwrap();
    let p = write(src.path(), "phone.jpg", &jpeg_rotated(40, 20));
    assert_eq!(dims(&make(&p).unwrap().unwrap()), (20, 40));
}

#[test]
fn transparency_keeps_png_and_opaque_becomes_jpeg() {
    let src = tempfile::tempdir().unwrap();
    let clear = write(
        src.path(),
        "logo.png",
        &png(RgbaImage::from_pixel(300, 300, Rgba([10, 20, 30, 0]))),
    );
    let solid = write(
        src.path(),
        "shot.png",
        &png(RgbaImage::from_pixel(300, 300, Rgba([10, 20, 30, 255]))),
    );
    let t = make(&clear).unwrap().unwrap();
    assert_eq!((t.mime, dims(&t)), ("image/png", (256, 256)));
    assert_eq!(make(&solid).unwrap().unwrap().mime, "image/jpeg");
}

#[test]
fn the_content_decides_not_the_extension() {
    let src = tempfile::tempdir().unwrap();
    // A PNG named .jpg previews; text named .jpg doesn't, and that is no error.
    let png_as_jpg = write(
        src.path(),
        "wrong.jpg",
        &png(RgbaImage::from_pixel(8, 8, Rgba([1, 2, 3, 255]))),
    );
    let text = write(src.path(), "notes.jpg", b"not a picture at all");
    let truncated = write(src.path(), "cut.jpg", &jpeg(400, 400)[..300]);
    assert!(make(&png_as_jpg).unwrap().is_some());
    assert_eq!(make(&text).unwrap(), None);
    assert_eq!(make(&truncated).unwrap(), None);
}

#[test]
fn only_image_extensions_are_tried() {
    assert!(can_preview(Path::new("a/IMG.JPEG")));
    assert!(can_preview(Path::new("scan.tiff")));
    assert!(!can_preview(Path::new("VID_0007.mov")));
    assert!(!can_preview(Path::new("IMG_1.HEIC")));
    assert!(!can_preview(Path::new("README")));
}

#[test]
fn a_missing_source_is_an_error_and_not_remembered() {
    let cache = tempfile::tempdir().unwrap();
    let thumbs = Thumbnails::new(cache.path().join("t"), DEFAULT_MAX_BYTES);
    assert!(thumbs.get(&cache.path().join("gone.jpg")).is_err());
    assert!(!cache.path().join("t").exists());
}

#[test]
fn the_source_is_only_read() {
    let src = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let p = write(src.path(), "a.jpg", &jpeg(600, 400));
    let before = (listing(src.path()), fs::read(&p).unwrap());
    let thumbs = Thumbnails::new(cache.path().join("thumbnails"), DEFAULT_MAX_BYTES);
    assert!(thumbs.get(&p).unwrap().is_some());
    assert!(thumbs.get(&p).unwrap().is_some());
    assert_eq!((listing(src.path()), fs::read(&p).unwrap()), before);
    assert_eq!(listing(&cache.path().join("thumbnails")).len(), 1);
}

#[test]
fn a_hit_answers_the_same_bytes_and_a_changed_source_a_new_one() {
    let src = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let thumbs = Thumbnails::new(cache.path().to_path_buf(), DEFAULT_MAX_BYTES);
    let p = write(src.path(), "a.jpg", &jpeg(600, 400));
    let first = thumbs.get(&p).unwrap().unwrap();
    assert!(matches!(thumbs.lookup(&p).unwrap(), Lookup::Found(Some(t)) if t == first));
    write(src.path(), "a.jpg", &jpeg(300, 600));
    assert!(matches!(thumbs.lookup(&p).unwrap(), Lookup::Missing(_)));
    assert_eq!(dims(&thumbs.get(&p).unwrap().unwrap()), (128, 256));
}

#[test]
fn a_file_without_preview_is_remembered() {
    let src = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let thumbs = Thumbnails::new(cache.path().to_path_buf(), DEFAULT_MAX_BYTES);
    let p = write(src.path(), "broken.png", b"\x89PNG garbage");
    assert_eq!(thumbs.get(&p).unwrap(), None);
    assert!(matches!(thumbs.lookup(&p).unwrap(), Lookup::Found(None)));
    let files = listing(cache.path());
    assert!(files.len() == 1 && files[0].ends_with(".none"), "{files:?}");
    // Not an image's extension: nothing is opened or written.
    let video = write(src.path(), "clip.mp4", b"....");
    assert!(matches!(
        thumbs.lookup(&video).unwrap(),
        Lookup::Found(None)
    ));
    assert_eq!(listing(cache.path()).len(), 1);
}

#[test]
fn the_least_recently_used_go_first_when_full() {
    let src = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    // Every tiny thumbnail costs one block: room for three.
    let thumbs = Thumbnails::new(cache.path().to_path_buf(), 3 * BLOCK);
    let p: Vec<PathBuf> = ["a", "b", "c", "d"]
        .iter()
        .map(|n| write(src.path(), &format!("{n}.jpg"), &jpeg(8, 8)))
        .collect();
    for x in &p[..3] {
        thumbs.get(x).unwrap().unwrap();
    }
    assert_eq!(thumbs.size(), 3 * BLOCK);
    // `a` is used again, so `b` and `c` are now the oldest.
    assert!(matches!(
        thumbs.lookup(&p[0]).unwrap(),
        Lookup::Found(Some(_))
    ));
    thumbs.get(&p[3]).unwrap().unwrap();
    // Over the limit: down to 90 % of it, oldest first.
    let found = |x: &Path| matches!(thumbs.lookup(x).unwrap(), Lookup::Found(Some(_)));
    assert_eq!(
        p.iter().map(|x| found(x)).collect::<Vec<_>>(),
        [true, false, false, true]
    );
    assert_eq!(thumbs.size(), 2 * BLOCK);
    assert_eq!(listing(cache.path()).len(), 2);
}

#[test]
fn a_restart_finds_the_cache_as_it_was() {
    let src = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let p = write(src.path(), "a.jpg", &jpeg(600, 400));
    let first = Thumbnails::new(cache.path().to_path_buf(), DEFAULT_MAX_BYTES);
    let t = first.get(&p).unwrap().unwrap();
    // Left by a crash long ago, and a file someone else put there.
    let stale = write(cache.path(), ".0123.1-0.tmp", b"half");
    File::options()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(SystemTime::now() - 2 * STALE_TEMP)
        .unwrap();
    write(cache.path(), "keep-me.txt", b"x");
    let again = Thumbnails::new(cache.path().to_path_buf(), DEFAULT_MAX_BYTES);
    assert_eq!(again.size(), cost(t.bytes.len() as u64));
    assert!(matches!(again.lookup(&p).unwrap(), Lookup::Found(Some(x)) if x == t));
    assert!(!stale.exists());
    assert!(cache.path().join("keep-me.txt").exists());
}
