//! filebridge CLI. Phase 0: scan names, build test trees, and benchmark renaming in transit.
//!
//! There is no `core` crate yet, so this binary wires `scanner`, `names` and
//! `transport-rclone` directly; Phase 1 moves that wiring behind `core`.

mod bench;
mod fixture;
mod prepare;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "filebridge", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List the names the NFC rule would correct. Read-only.
    Scan {
        source: PathBuf,
        /// Print every corrected name with its bytes.
        #[arg(long)]
        show: bool,
    },
    /// Create a synthetic test tree where some names are NFD. Refuses a non-empty folder.
    Fixture(fixture::Args),
    /// Compare a plain `sync/copy` (baseline) with plan A (per-file rename) and plan B
    /// (temp folder + batch rename + move). Never writes to the source.
    Bench(bench::Args),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Scan { source, show } => prepare::print_scan(&source, show),
        Cmd::Fixture(args) => fixture::run(&args),
        Cmd::Bench(args) => bench::run(args).await,
    }
}
