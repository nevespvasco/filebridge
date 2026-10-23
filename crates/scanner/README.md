# crates/scanner

Walks the source, **reads names as bytes**, and builds manifest entries. **Lands in:** Phase 0 (basic), Phase 2 (hashing).

## Responsibility
- Recursive walk (parallel, e.g. `jwalk`/`ignore`), never assuming UTF-8 names
  (`OsStrExt::as_bytes` on Unix).
- Per entry: raw path bytes, decoded name (if valid UTF-8), size, mtime, is_dir.
- Running totals (`ScanProgress`: files, bytes, current path).
- Hash source files (SHA-256, ADR 0017) streaming, with a bounded worker pool.
- Produce `names::Entry` values and hand them to `store` in batches.
- A folder's size per item directly inside it (`folder_sizes`): read-only, links not
  followed, stoppable, with progress (docs/design/specs/D02).
- The disks mounted here that files can be sent from (`volumes`): Linux mountinfo + sysfs,
  macOS `/Volumes`, Windows drive letters; free space through `statvfs` on Unix.

## Allowed dependencies
`names` (for `Entry`), hashing crates, `libc` (Unix, for `statvfs`). No `store` dependency — `core` persists the output.

See [docs/04-name-correction.md](../../docs/04-name-correction.md).
