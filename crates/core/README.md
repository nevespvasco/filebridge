# crates/core

The heart of the app: **Job, pipeline, states, Registry, progress events.** **Lands in:** Phase 1.

## Responsibility
- `Job` and `JobKind` (`Transfer`, `Repair`).
- The 6-stage pipeline: source & destination → analyse → fix names → transfer → verify → update tool.
- Job state machine, persistence of every transition via `store`, resume on startup.
- Retry with backoff (30 s → … → 15 min cap, unlimited).
- `Registry` of transports, integrations and general name rules.
- Typed progress events (`specta::Type`) consumed by the UI and CLI.
- Sleep inhibitor lifecycle (via a small per-OS module).

## Planned public API (draft)
```rust
pub struct Engine { /* registry, store, secrets, event bus */ }
impl Engine {
    pub async fn create_job(&self, sources: Vec<PathBuf>, profile: ProfileId) -> Result<JobId>;
    pub async fn analyse(&self, job: JobId) -> Result<()>;
    pub async fn decide_issue(&self, issue: IssueId, d: Decision) -> Result<()>;
    pub async fn start(&self, job: JobId, opts: SendOptions) -> Result<()>;
    pub async fn pause(&self, job: JobId) -> Result<()>;
    pub async fn resume(&self, job: JobId) -> Result<()>;
    pub async fn cancel(&self, job: JobId) -> Result<()>;
    pub async fn verify(&self, job: JobId) -> Result<()>;
    pub fn subscribe(&self) -> broadcast::Receiver<Event>;
}
```

## Allowed dependencies
`transport-api`, `integration-api`, `names`, `scanner`, `store`, `secrets`.
**Never** a concrete `transport-*` / `integration-*` crate.

See [docs/02-architecture.md](../../docs/02-architecture.md) and [docs/03-extension-contracts.md](../../docs/03-extension-contracts.md).
