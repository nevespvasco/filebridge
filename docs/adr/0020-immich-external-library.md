# ADR 0020 — Immich as an external library fed over SFTP, scanned through its API

- Status: Accepted
- Date: 2026-10-23

## Context
Phase 3 asks for a second `Integration` ([09 — Roadmap](../09-roadmap.md)), and its gate is that
a new plugin enters without changing `core`. Immich (a self-hosted photo server) was the first
candidate in [03 — Extension contracts](../03-extension-contracts.md).

Immich takes files in two ways:

1. **Its upload API** (`POST /api/assets`, what its phone app uses): Immich stores each file in
   its own storage layout (by user and date) and deduplicates by checksum. Sending this way is a
   `Transport`, not an `Integration`: it replaces how bytes move. It can't hash a file back at
   the destination, so verification would need a new path; an interrupted upload starts over;
   and one HTTP request per photo is slower than SFTP on the local network for a big first
   migration.
2. **An external library**: an administrator points Immich at folders (its *import paths*), and
   a library scan (`POST /api/libraries/{id}/scan`) imports what is new or changed. The files stay
   where they are, in the folders the user chose; Immich only reads them and never moves or
   deletes them.

The second fits what filebridge already does well (SFTP with resume, SHA-256 verification at
the destination, names fixed in transit, the manifest), and it keeps the photos usable without
Immich. The API has no scan by path: a scan always covers the whole library, and it runs in
the background after the request is accepted.

## Decision
- **`crates/integration-immich` is an `Integration`.** The profile's transport (SFTP, or a local
  folder when Immich reads a folder of this machine) writes into a folder inside one of the
  library's import paths; after the transfer the integration asks Immich to scan the library.
  No new `Transport`, no change in `core`.
- **Settings:** the server's address (HTTPS, except on this machine, as for Nextcloud's WebDAV
  mode: the key travels with every request), the API key as a `KeyringRef`
  (`filebridge/<profile>/api-key`, new `SecretKind::ApiKey`; ADR 0004), the library id, and the
  destination's root **as Immich sees it** (`import_path`), because Immich in Docker sees the
  host's `/srv/photos` as e.g. `/mnt/photos`. Empty means the same path.
- **The key must be an administrator's** (library endpoints are admin-only) with at least
  `library.read` and `library.update`.
- **Preflight** (on every send and connection test): Immich answers (`/api/server/ping`), the
  key signs in and may read the library, the library exists, `import_path` is inside one of its
  import paths, and Immich can read that import path (`/validate`; a warning, not a failure).
- **After the transfer:** one scan request for the library. A refused or missing key, missing
  permission or deleted library is a fatal integration error (the files are sent; the job ends
  with a warning); `429`, `5xx` and no answer are retried.
- **Wizard** (`add_immich`, `filebridge profile add-immich`, the window's "Immich" kind): it
  keeps the key in the keyring under a fresh profile id, looks at the destination folder, runs
  the preflight checks, and picks the library by the folder when none is named. It saves the
  profile only when nothing failed; otherwise the key is removed again.

## Consequences
- Phase 3's gate holds for this plugin: `git diff main -- crates/core` is empty. Besides the new
  crate, the change touched the host's registry and wizard, one ipc operation, one secret kind,
  the CLI and the UI's form.
- Each send scans the whole library. Immich's scan only imports new or changed files, but it
  still looks at every file of the library, so a big library takes a while to settle after a
  small send. filebridge doesn't wait for the scan to finish.
- Immich identifies an external-library photo by its path. "Repair destination" renames show
  up in Immich as new photos, and the old entries go offline; albums, favourites and
  descriptions stay with the old ones. The Repair confirmation says so.
- A file whose name matches one of the library's exclusion patterns, or whose type Immich doesn't
  read, is sent and verified but never appears in Immich. filebridge doesn't warn about it yet.
- Upload through Immich's API (option 1) stays open as a future `Transport` for those without
  SSH access to the server.
