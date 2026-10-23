# ADR 0017 — The manifest uses SHA-256

- Status: Accepted
- Date: 2026-10-23
- Closes the open decision "xxHash or SHA-256 for the manifest?" in
  [10](../10-risks-and-open-decisions.md#open-decisions)

## Context
Verification compares each file at the destination with the source's hash in the manifest
([02](../02-architecture.md), stage 5). The plan left the algorithm open: xxHash (XXH3-128) is
much faster to compute, and SHA-256 is the standard one that servers can compute.

What the code does since Phase 2:
- The engine hashes every source file with SHA-256 (`crates/core/src/run.rs`, `sha2`), and the
  store keeps it per entry with `hash_algo = 'sha256'`.
- `Transport::hash_many` asks the destination for the same hash. Over SFTP the server computes it
  (`sha256sum`). Nextcloud over WebDAV has no SHA-256, so rclone downloads the file and hashes it.
- rclone has no XXH3-128 for either backend. `HashAlgo::Xxh3_128` and `TransportCaps::HASH_XXH`
  existed in the contract, but every transport refused it and nothing asked for it.

The time is spent on the network and the disks (a server-side `sha256sum`, or a download), not on
hashing. A faster algorithm on the source side would not make a verification noticeably shorter.

## Decision
- The manifest's hash is **SHA-256**, on the source and at the destination.
- `HashAlgo` keeps only `Sha256`, and `TransportCaps::HASH_XXH` is removed (its bit stays
  unused). The `hash_algo` column stays, so a future algorithm can sit next to SHA-256 in old
  manifests.
- Adding another algorithm needs a new ADR, and a destination that can compute it.

## Consequences
- One algorithm end to end: a server hash and a downloaded file compare the same way.
- The `xxhash-rust` dependency planned in [01](../01-stack.md) is not needed.
- Hashing a large source on a slow CPU (a phone) costs more than XXH3 would. Accepted: the send
  and the destination side still dominate.
