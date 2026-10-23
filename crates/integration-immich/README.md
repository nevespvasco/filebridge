# crates/integration-immich

Second `Integration`: **Immich** as an external library. **Lands in:** Phase 3
([ADR 0020](../../docs/adr/0020-immich-external-library.md)).

## Responsibility
- The profile's transport (SFTP, or a local folder) writes into a folder inside one of an Immich
  external library's import paths. After the send, one `POST /api/libraries/{id}/scan` asks
  Immich to import what is new or changed (the API has no scan by path: the whole library is
  scanned, in the background). Immich only reads the folder.
- Then the step **follows the scan**: Immich's `library` job queue (`GET /api/queues/library`,
  or `GET /api/jobs` on older Immich) until it is idle, with backoff (2 s doubling to 30 s) and
  progress (assets imported so far), for an hour at most (`ScanWait`). Once Immich accepted the
  scan the step saves a checkpoint (`IntegrationSink::checkpoint`), so a retry or a resumed job
  waits for that scan instead of asking for another. A paused queue shows as waiting
  (`immich.queue-paused`); past the hour (`immich.scan-unfinished`) or with a key that may not
  read the queue (`immich.scan-not-followed`) the step ends with a problem, not a failure.
- **Files the scan will skip** (`skip`, `Integration::will_skip`): names matching the library's
  exclusion patterns (`immich.excluded`), hidden names below the import path
  (`immich.hidden`), and extensions Immich doesn't import (`immich.unsupported`; the server's
  `/server/media-types`, or Immich's own lists). `core` puts them in the analysis' preflight as
  a `skipped` warning; the after-transfer report lists them again as problems. Never blocks.
- Preflight and the profile wizard's checks, each a `Check`: `server` (`/api/server/ping`, the
  version), `api-key` (the key signs in and may read libraries: an administrator's), `library`
  (it exists; with no id, the one library whose import paths hold the folder), `import-path`
  (the folder is inside one of them), `readable` (Immich can read that import path,
  `/validate`; a warning only).
- The wizard's `same-folder` check (`Immich::same_folder`): the host writes a probe file
  (`.filebridge-probe-<id>`, hidden, so no scan imports it) into the destination's root, Immich's
  path validation is asked about `<import_path>/<probe>` (a file it finds is "Not a directory",
  one it doesn't is ENOENT), and the host removes the probe with `Transport::remove_probe`. Not
  found: the profile isn't saved, and the window explains it (pt/en). A destination folder that
  doesn't exist yet can't be probed (a warning).
- No name rules of its own: Immich imports any name its file system holds.

## Modules
| Module | What |
|---|---|
| `settings` | `ImmichSettings { server, api_key: KeyringRef, library_id, import_path }`, `server_url` (HTTPS except on loopback, no credentials, a trailing `/api` dropped), `valid_library_id` (a UUID, checked before it goes into a URL), `clean_path`, `covers`, and the profile check every save runs (SFTP with an absolute root, or a local folder) |
| `api` | `Api` over reqwest (rustls, the OS's and the bundled roots): `ping`, `version`, `libraries`, `library`, `validate`, `scan`, `library_queue`, `media_types`; the key only in the `x-api-key` header. Statuses become an `ApiErrorKind`: `401` Auth, `403` Forbidden, `400`/`404` NotFound, `429`/`5xx`/no answer Busy or Unreachable (retried) |
| `inspect` | The checks above, shared by `preflight` and `Immich::set_up` |
| `skip` | `Skips`: which files the scan passes over (a small glob matcher like fast-glob's, case-insensitive) |
| `lib` | `Immich` (the `Integration`; reads the key through the host's `SecretResolver`), `set_up` (the wizard's plugin side: normalises what was typed, runs the checks, builds the settings) and `same_folder` |
| `fake` | `MockImmich` (feature `fake`): a small HTTP/1.1 server on `127.0.0.1` that answers those calls like Immich and records them, for the tests here and in `crates/host`. It simulates the library queue (a scan stays busy for a few looks; current or legacy queue API, paused, refused), media types, and what Immich's file system holds (`mount`) for the path validation |

`import_path` is the destination's root as Immich sees it. When Immich runs in Docker it differs
from the SFTP path (`/srv/photos` on the host, mounted as `/mnt/photos`); empty in the wizard
means the same path.

The wizard's host side (`crates/host/src/immich.rs`) keeps the key in the keyring under the new
profile's id (`filebridge/<id>/api-key`), lists the destination folder and its space with the
host's transport, calls `set_up`, writes and removes the probe for `same_folder`, and saves the
profile only when nothing failed (otherwise the key goes again). Deleting the profile removes
the key with its other secrets.

The API key needs `library.read`, `library.update` and `queue.read` (`job.read` before Immich 2).

## Tests
`tests/mock_server.rs` runs the wizard, the preflight and the after-transfer step against
`MockImmich`: a library picked by the folder or by id, ambiguous and missing libraries, a wrong
key and a non-admin key, an unreadable folder, a server that doesn't answer, a busy Immich
(retried) and a refused scan (not); following the scan through both queue APIs, a resumed step
that waits instead of asking again, a paused queue (timeout), a key that may not read the
queue; skipped files before and after the send, with and without the server's media types; and
the probe found, not found, and unanswered. `crates/host` sends a folder through the engine to a
local destination and checks that the send ends with exactly one scan request, and that the
wizard's probe passes, fails (nothing saved) and leaves only the files that were there.

## Not yet
- A real Immich server: everything is tested against the mock.

## Allowed dependencies
`integration-api`, `transport-api`, `reqwest`, `serde`, `serde_json`, `tokio`, `async-trait`,
`url`. Never `filebridge-secrets` nor another plugin: the key is read through the
`SecretResolver` the host injects.

See [ADR 0020](../../docs/adr/0020-immich-external-library.md).
