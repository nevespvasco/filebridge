# ADR 0027 — Thumbnails of source images, made by the engine and cached in the app's cache

- Status: Accepted
- Date: 2026-10-23

## Context
D05 left "a thumbnail preview of the source image (F13)" for later: it proves nothing, but in
a Files tab of 18 000 photos a picture finds a file faster than its name. The source is local
to the engine (the service, or the app itself on phones); the window may not be able to read it
(ADR 0008), and on phones the files sit under `/storage/emulated/0` behind "All files access".
Nothing may ever be written next to the source.

## Decision
- A new crate, **`crates/thumbnail`**, decodes a local image with the `image` crate (pure Rust
  codecs only, default features off: JPEG, PNG, GIF, WebP, BMP, TIFF), applies its EXIF
  orientation, shrinks it to fit 256×256 and encodes it as JPEG (PNG when it has transparency).
  Sources over 256 MB and decodes needing more than 256 MB of memory get no preview.
- It keeps the results in **`<cache>/thumbnails/`**: one file per source, named by a hash of
  the source's path, size and modification time (a changed file gets a new one), at most
  **128 MB**, the least recently used removed first (down to 90 %). A file that can't be
  previewed leaves an empty marker so it isn't decoded again; a source that can't be read now
  leaves nothing. The folder is user-only (0700).
- `<cache>` is the OS cache directory (`$XDG_CACHE_HOME/filebridge`, `~/Library/Caches/filebridge`,
  `%LOCALAPPDATA%\filebridge\cache`), `cache/` inside an explicit `--data-dir`, or on phones the
  app's own cache folder (`HostOptions.cache_dir`).
- `crates/host` wires it: one ipc operation, **`entry_thumbnail(job_id, idx) ->
  Option<EntryThumbnail>`** (`mime` and the bytes in base64). `core` only says which local file
  an Entry is (`Engine::entry_source_file`; none for folders and Repair jobs) and never sees
  the image code. At most two thumbnails are decoded at once, on blocking threads.
- **Videos, HEIC and RAW photos get no preview**: every way to read a video frame or HEIC
  needs a C library (ffmpeg, libheif) that phones and the sidecar-free desktop build don't
  have. The window shows the Category icon for them.
- The window asks only for rows of Category Image that have stayed on screen for 150 ms, at
  most three at a time (newest first), and keeps the last 400 in memory. A row has the same box
  with the icon or the picture, so nothing shifts when a preview arrives.

## Consequences
- The bytes go over the socket as base64 (about 15 kB per row): no file path for the window to
  read, no Tauri asset-protocol scope to widen, and the same on phones.
- `image` is a dependency of `crates/thumbnail` only (not in `[workspace.dependencies]`).
- Deleting the cache folder is always safe: previews are made again on demand.
- A video preview later means a new decoder in `crates/thumbnail`; the operation and the
  window don't change.
