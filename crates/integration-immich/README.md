# crates/integration-immich

Second `Integration`: **Immich** as an external library. **Lands in:** Phase 3
([ADR 0020](../../docs/adr/0020-immich-external-library.md)).

## Responsibility
- The profile's transport (SFTP, or a local folder) writes into a folder inside one of an Immich
  external library's import paths. After the send, one `POST /api/libraries/{id}/scan` asks
  Immich to import what is new or changed (the API has no scan by path: the whole library is
  scanned, in the background). Immich only reads the folder.
- Preflight and the profile wizard's checks, each a `Check`: `server` (`/api/server/ping`, the
  version), `api-key` (the key signs in and may read libraries: an administrator's), `library`
  (it exists; with no id, the one library whose import paths hold the folder), `import-path`
  (the folder is inside one of them), `readable` (Immich can read that import path,
  `/validate`; a warning only).
- No name rules of its own: Immich imports any name its file system holds.

## Modules
| Module | What |
|---|---|
| `settings` | `ImmichSettings { server, api_key: KeyringRef, library_id, import_path }`, `server_url` (HTTPS except on loopback, no credentials, a trailing `/api` dropped), `valid_library_id` (a UUID, checked before it goes into a URL), `clean_path`, `covers`, and the profile check every save runs (SFTP with an absolute root, or a local folder) |
| `api` | `Api` over reqwest (rustls, the OS's and the bundled roots): `ping`, `version`, `libraries`, `library`, `validate`, `scan`; the key only in the `x-api-key` header. Statuses become an `ApiErrorKind`: `401` Auth, `403` Forbidden, `400`/`404` NotFound, `429`/`5xx`/no answer Busy or Unreachable (retried) |
| `inspect` | The checks above, shared by `preflight` and `Immich::set_up` |
| `lib` | `Immich` (the `Integration`; reads the key through the host's `SecretResolver`) and `set_up`, the wizard's plugin side: normalises what was typed, runs the checks, builds the settings |
| `fake` | `MockImmich` (feature `fake`): a small HTTP/1.1 server on `127.0.0.1` that answers those calls like Immich and records them, for the tests here and in `crates/host` |

`import_path` is the destination's root as Immich sees it. When Immich runs in Docker it differs
from the SFTP path (`/srv/photos` on the host, mounted as `/mnt/photos`); empty in the wizard
means the same path.

The wizard's host side (`crates/host/src/immich.rs`) keeps the key in the keyring under the new
profile's id (`filebridge/<id>/api-key`), lists the destination folder and its space with the
host's transport, calls `set_up`, and saves the profile only when nothing failed (otherwise the
key goes again). Deleting the profile removes the key with its other secrets.

## Tests
`tests/mock_server.rs` runs the wizard, the preflight and the after-transfer step against
`MockImmich`: a library picked by the folder or by id, ambiguous and missing libraries, a wrong
key and a non-admin key, an unreadable folder, a server that doesn't answer, a busy Immich
(retried) and a refused scan (not). `crates/host` sends a folder through the engine to a local
destination and checks that the send ends with exactly one scan request.

## Not yet
- Waiting for Immich to finish the scan, with progress (its job queues).
- Warning about sent files Immich will skip: names matching the library's exclusion patterns, or
  file types it doesn't import.
- Checking that `import_path` really is the SFTP folder (only that the library holds it and
  Immich can read it).

## Allowed dependencies
`integration-api`, `transport-api`, `reqwest`, `serde`, `serde_json`, `tokio`, `async-trait`,
`url`. Never `filebridge-secrets` nor another plugin: the key is read through the
`SecretResolver` the host injects.

See [ADR 0020](../../docs/adr/0020-immich-external-library.md).
