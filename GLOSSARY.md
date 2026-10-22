# Glossary

The words the code and the docs use for filebridge's concepts. Architecture terms (module,
interface, seam, adapter) are used in their usual sense.

**Job**: one send, from the sources the user picked to a Profile's destination. Goes through the
Stages and the Job lifecycle's states; everything about it is persisted in SQLite.
Shown to the user as a **Transfer** (pt: *Transferência*). Never called an "upload": filebridge
owns no store to upload to.

**Source**: a file or folder the user picked for a Job. filebridge only reads it; nothing at a
Source is ever moved, renamed or deleted.

**Label**: the name the user gives a Job ("Photos 2019"). Optional; without one the Job is
shown by its first Source's name.

**Job group**: which of five sets a Job's state falls in, for the user: active, needs you, not
sent, finished, cancelled. Every state is in exactly one group.

**Send again**: a new Job with the same Sources and options as an earlier one, to the same or
another Profile, reusing the earlier Job's Decisions. Only files that are new or changed travel.

**Stage**: one of the six steps of a Job (source & destination, analyse, fix names, transfer,
verify, update tool). Docs/02.

**Batch**: one page of a Job's Pending Entries (up to `batch_size`, 500 by default) sent with one
`Transport::transfer` call, from however many Sources. Its Entries go `Partial` before the call
and `Done` as the transport reports each arrival (`crates/core/src/batch.rs`). Verify asks
`Transport::hash_many` in batches too, capped by files and bytes.

**Job lifecycle**: which operation may run in which state, which state may follow which, and how a
state change is recorded (with the finished Stage's outcome). The rules are pure data in
`crates/model/src/rules.rs`; `crates/core/src/lifecycle.rs` enforces and records them.

**Stage outcome**: what a finished Stage leaves for the end of the Job: its warnings, and whether
it was a verify round that sends files again. Saved with the state change that ends the Stage.

**Manifest**: one row per Entry of a Job: raw source path, size, destination path, transfer and
verify state.

**Entry**: one source file or directory, its name as raw bytes.

**Category**: a coarse kind of file, from its name's extension: image, video, audio, document,
archive, code, or other. Used to show what a Job contains; it never changes how a file is sent.

**Entry status**: one word for where an Entry stands, merging its transfer and verify states:
not sent, sending, sent, verified, different, missing, couldn't verify, failed, skipped. A
**problem** is different, missing, couldn't verify or failed.

**Name rule** (`NameRule`): checks one name and proposes a fix (NFC, forbidden characters,
reserved names…). General rules live in `names`; an Integration adds its own.

**Issue**: what a Name rule found wrong with one Entry's name, with its proposal.

**Decision** / **Choice**: what the user chose for an Issue: accept, keep the original (leave
that rule out), edit (a name of their own), or skip the Entry. `Decision` is how the store keeps
it; `Choice` is how the Name plan reads it.

**Name plan**: the module that turns Entries and Decisions into the destination path of every
Entry: proposes (with collisions and undecodable names), checks a Choice when it is made, and
resolves the final paths at Send. `crates/names/src/plan.rs`.

**Profile**: a saved destination: a Destination, an optional Integration with its settings, and
the target's naming family.

**Destination**: where files go: a Transport id, a backend (local, SFTP) and a root.

**Transport**: the contract that moves bytes to a Destination (rclone, the local-disk fake).
Every adapter passes the contract suite (`transport-api`, feature `contract`).

**Integration**: the contract for the tool that owns the destination (Nextcloud): its name rules,
its check of a Profile, preflight, and the update after a send.

**Operation catalogue**: the one list of everything a client can ask the service
(`crates/ipc/src/operations.rs`), from which the wire request, the `Operations` trait, the
server's dispatch and the desktop's Tauri commands are generated.

**Job log**: everything recorded about one Job, in order: creation, options, analysis,
decisions, state changes, retries, transfer rounds, verify rounds, the Integration's update and
the end. Kept for good, readable after the window was closed.

**Activity**: the milestones of every Job (created, started, retrying, finished, failed) and
Profile changes, newest first. A short summary drawn from the Job logs, not a log of its own.

**Report**: the export of one finished Job: its summary and every Entry with source path,
destination path, size, Entry status, the Name rules that changed it, and its hash. The proof
of a migration that can leave the app.

**Reclaimable**: said of a Source whose every file a completed Job sent and verified identical,
with nothing skipped and nothing changed at the Source since. filebridge shows a reclaimable
Source and opens it in the file manager; it never frees the space itself (ADR 0011).
