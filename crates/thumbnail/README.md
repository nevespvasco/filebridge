# crates/thumbnail

Small previews of source images for the Files tab and the Entry detail
([D05](../../docs/design/specs/D05-files-and-entry-detail.md), ADR 0027). **Lands in:** Phase 2.

## Responsibility
- Decode a local image (JPEG, PNG, GIF, WebP, BMP, TIFF), apply its EXIF orientation, shrink
  it to fit 256×256, and encode it (JPEG, or PNG when it has transparency).
- Keep the results in a cache folder of bounded size (least recently used evicted first),
  keyed by the source's path, size and modification time. Files that can't be previewed are
  remembered too, so they aren't decoded again.
- Only **reads** the source. Nothing is ever written next to it.

Videos, HEIC and RAW photos get no preview (no dependency-light decoder): the window shows the
Category icon.

## Allowed dependencies
`image` (pure Rust codecs only, default features off) and `sha2`. No other filebridge crate:
`host` resolves an Entry to its source path and calls this crate.
