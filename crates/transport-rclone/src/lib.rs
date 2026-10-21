//! `Transport` implementation over rclone's `rc` API.
//!
//! [`Rcd`] supervises an `rclone rcd` sidecar bound to localhost with per-start random
//! credentials and an in-memory config (no `rclone.conf` on disk). Backends are passed as
//! connection strings built from a [`Destination`](filebridge_transport_api::Destination).
//!
//! Two transfer strategies are implemented so Phase 0 can compare them:
//! - [`Strategy::PerFile`] (plan A): one `operations/copyfile` per file, straight to the
//!   corrected name.
//! - [`Strategy::TempThenRename`] (plan B): `sync/copy` with original names into a temp folder
//!   next to the destination, batch rename there, then `sync/move` into place.

mod client;
pub mod fs;
mod rcd;
mod rename;
pub mod stats;
mod transport;

pub use client::RcClient;
pub use rcd::Rcd;
pub use transport::{RcloneTransport, Strategy};
