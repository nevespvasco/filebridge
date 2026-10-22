# Glossary

The words the code and the docs use for filebridge's concepts. Architecture terms (module,
interface, seam, adapter) are used in their usual sense.

**Job**: one send, from the sources the user picked to a Profile's destination. Goes through the
Stages and the Job lifecycle's states; everything about it is persisted in SQLite.

**Stage**: one of the six steps of a Job (source & destination, analyse, fix names, transfer,
verify, update tool). Docs/02.

**Job lifecycle**: which operation may run in which state, which state may follow which, and how a
state change is recorded (with the finished Stage's outcome). The rules are pure data in
`crates/model/src/rules.rs`; `crates/core/src/lifecycle.rs` enforces and records them.

**Stage outcome**: what a finished Stage leaves for the end of the Job: its warnings, and whether
it was a verify round that sends files again. Saved with the state change that ends the Stage.

**Manifest**: one row per Entry of a Job: raw source path, size, destination path, transfer and
verify state.

**Entry**: one source file or directory, its name as raw bytes.

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
