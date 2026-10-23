# crates/integration-api

The `Integration` trait and its types. **Lands in:** Phase 1.

## Contents
- `Integration`: `id`, `required_caps`, `name_rules`, `preflight(&JobContext)`,
  `after_transfer(&JobContext, &[RemotePath], IntegrationSink) -> AfterTransferReport`.
- `JobContext { job_id, profile: ProfileView, caps, checkpoint }`. `ProfileView` holds the `Destination` and
  the integration's own `settings` (JSON, opaque to `core`; decode with `settings_as::<T>()`).
  No SQLite, no keyring, no secrets.
- `IntegrationError { message, retryable }` (like `TransportError`, and `From` it).
- `IntegrationEvent` / `IntegrationSink`: typed progress for integration steps — `Stage(key)`,
  `Progress(StepProgress)`, `Waiting(key)` ("waiting for disk"), `Problem(Problem)`. Separate
  from `transport_api::ProgressEvent` because permissions and indexing are not byte transfers.
  Stage and waiting values are i18n keys, so the UI translates them.
- Checkpoints: `IntegrationSink::checkpoint(state).await` asks the host to persist a small JSON
  state for the job and returns once it is saved; when the step runs again (a retry, or the job
  resumed) `JobContext::checkpoint` holds the last one. `core` keeps them in the job's log
  (`integration_checkpoint`). Immich uses it not to ask for a second library scan.
- `AfterTransferReport { updated, items, problems }`; a `Problem` has a stable `code` (also its
  i18n key), the tool's message, a destination-relative `path` and a `FixAction`
  (`RepairDestination`, `Retry`).
- Re-exports `TransportCaps`, `PreflightReport`, `Check`, `CheckStatus`, `Destination`,
  `RemotePath`, `TransportError` from `transport-api` and `NameRule` from `names`.

## Rules
- An integration declares its own name rules, per profile (`name_rules(settings)`): a rule may
  depend on how that server is set up (Nextcloud's Windows compatibility, say).
- `after_transfer` receives the **exact list of changed paths**.
- `will_skip(ctx, files)` (default: nothing) lists, as `Problem`s, the files a send is about to
  write that the tool won't take (Immich: excluded or unsupported types). `core` asks it once
  the job's names are known and adds a warning check to the integration's preflight; the send
  is never blocked by it.
- `required_caps()` is checked against the transport when a profile is created.
- Problems that don't stop the step (an `incompatible encoding` entry) go in the report; `Err`
  means the step itself failed, and `retryable` tells `core` whether to back off and retry.

## Allowed dependencies
`transport-api`, `names`, `async-trait`, `serde`, `serde_json` (opaque settings), `thiserror`,
`tokio` (`sync`, for the sink), `specta` (when the types first cross into the UI).

See [docs/03-extension-contracts.md](../../docs/03-extension-contracts.md).
