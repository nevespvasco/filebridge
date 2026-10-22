// Development only: `VITE_MOCK=1 pnpm dev` runs the UI in a plain browser against this in-memory
// fake of filebridge-service (through Tauri's `mockIPC`), for visual work. It is never bundled into
// the app: main.ts imports it only when VITE_MOCK is set. For real behaviour without rclone, run
// the app against `filebridge-service --fake-transport` instead (see README).
//
// It can't drift from the service: every command in the generated bindings needs a handler here
// with the command's own answer type (`Handlers`), and which operation is allowed in which job
// state comes from the engine's table (`JOB_OPERATIONS`), not from a copy.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { trimBoth, trimEnd } from "@/lib/format";
import { sshAccounts } from "@/stores/profiles";
import { setMockPicker, setMockSave, type PickOptions } from "@/lib/platform";
import { emit } from "@tauri-apps/api/event";
import {
  JOB_OPERATIONS,
  PROTOCOL_VERSION,
  SOURCE_CHILDREN_CAP,
  type AnalysisSummary_Serialize,
  type AppSettings_Serialize,
  type commands,
  type DecisionInput,
  type DoneView,
  type ErrorBody,
  type Event_Serialize,
  type FileCategory,
  type InFlightView_Serialize,
  type IssueDecision,
  type IssueView_Serialize,
  type JobState,
  type CategoryCount,
  type EntryDetail_Serialize,
  type EntryFilter,
  type EntryStatus,
  type EntryView_Serialize as EntryView,
  type JobFilter,
  type JobGroup,
  type JobSummary_Serialize,
  type LogEntry,
  type NextcloudWebdavRequest,
  type ReclaimStatus_Serialize,
  type RecentSource,
  type StorageView_Serialize as StorageView,
  type Profile_Serialize,
  type RuleCount,
  type SourceInfo_Serialize,
  type SourceProblem,
} from "@/bindings";

type Args = Record<string, unknown>;

/** A short random id, like the service's (`p-…` for profiles, `j-…` for jobs). */
const newId = (prefix: string) => `${prefix}-${crypto.randomUUID().slice(0, 8)}`;

const now = Date.now();
const profiles: Profile_Serialize[] = [
  {
    id: "p-nc",
    name: "Nextcloud casa — demo/Backup",
    destination: { transport: "rclone", backend: { kind: "sftp", host: "pve.example.lan", port: null, user: "root", key_file: null, known_hosts_file: null, password: null, key: null }, root: "/srv/nextcloud/data/demo/files/Backup" },
    integration: "nextcloud",
    integration_settings: { mode: "direct" },
    target: { family: "unix", case_sensitive: true, max_name_bytes: null, max_path_bytes: null },
  },
  {
    id: "p-disk",
    name: "Disco externo",
    destination: { transport: "rclone", backend: { kind: "local" }, root: "/media/disco/Backup" },
    integration: null,
    integration_settings: null,
    target: { family: "windows", case_sensitive: false, max_name_bytes: null, max_path_bytes: null },
  },
];

function counts(files: number, bytes: number, doneFrac: number) {
  const done = Math.floor(files * doneFrac);
  return { files, dirs: Math.ceil(files / 40), bytes, pending: files - done, done, failed: 0, skipped: 0, bytes_done: Math.floor(bytes * doneFrac), verified_ok: 0, mismatch: 0 };
}

const jobs: JobSummary_Serialize[] = [
  {
    id: "j-old",
    kind: "transfer",
    profile_id: "p-disk",
    state: "completed",
    error: null,
    attempt: 0,
    next_retry_at: null,
    created_at: now - 86_400_000,
    updated_at: now - 80_000_000,
    finished_at: now - 80_000_000,
    sources: ["/home/demo/Documentos"],
    options: { verify: true, concurrency: 8, target_dir: null },
    counts: { ...counts(1234, 4_500_000_000, 1), verified_ok: 1234 },
    label: "Documentos do escritório",
    again_of: null,
  },
];

// `VITE_MOCK_EMPTY=1`: start like a new user, with no destinations and no jobs.
if (import.meta.env.VITE_MOCK_EMPTY === "1") {
  profiles.length = 0;
  jobs.length = 0;
}

// The job log, as the engine records it (docs/design/specs/D07): parameters only.
const logs = new Map<string, LogEntry[]>();
/** App-level events (profile changes), with no job. */
const appLog: (LogEntry & { jobId: null })[] = [];
let nextLogId = 1;
function log(jobId: string, kind: string, data: Record<string, unknown>, at = Date.now()) {
  const n = (k: string) => Number(data[k] ?? 0);
  const level: LogEntry["level"] =
    (kind === "state" && data.to === "failed") || (kind === "finished" && data.state === "failed")
      ? "error"
      : kind === "retry" || (kind === "transfer_round" && n("failed") > 0) || (kind === "finished" && data.state === "completed_with_warnings") || (kind === "analysis" && n("scan_errors") > 0)
        ? "warn"
        : "info";
  const list = logs.get(jobId) ?? [];
  list.push({ id: nextLogId++, at, kind, level, data: data as LogEntry["data"] });
  logs.set(jobId, list);
}
appLog.push({ id: nextLogId++, at: now - 2 * 86_400_000, kind: "profile", level: "info", data: { action: "added", profile_id: "p-disk", name: "Disco externo" }, jobId: null });
if (jobs.some((j) => j.id === "j-old")) {
  const t = now - 86_400_000;
  log("j-old", "created", { sources: ["/home/demo/Documentos"], profile_id: "p-disk", options: { verify: true, concurrency: 8 } }, t);
  log("j-old", "analysis", { files: 1234, bytes: 4_500_000_000, scan_errors: 0, skipped_symlinks: 0 }, t + 20_000);
  log("j-old", "state", { from: "ready", to: "transferring", error: null }, t + 30_000);
  log("j-old", "retry", { attempt: 1, in_secs: 30, error: "connection reset by peer" }, t + 1_800_000);
  log("j-old", "transfer_round", { sent: 1234, failed: 0, bytes: 4_500_000_000, first_error: null }, t + 5_200_000);
  log("j-old", "verify", { checked: 1234, mismatch: 0, missing: 0, unverifiable: 0 }, t + 5_900_000);
  log("j-old", "finished", { state: "completed", warnings: 0, error: null }, now - 80_000_000);
}

// Issues are generated on demand, so a rule can have hundreds of thousands of them.
type RuleDef = { rule: string; total: number; auto: boolean; base: number };
const rulesByJob = new Map<string, RuleDef[]>();
const decisions = new Map<number, { decision: IssueDecision; custom_name: string | null }>();

function rulesFor(jobId: string): RuleDef[] {
  let r = rulesByJob.get(jobId);
  if (!r) {
    const base = rulesByJob.size * 10_000_000;
    r = [
      { rule: "nfc", total: 120_000, auto: true, base: base + 1 },
      { rule: "forbidden-chars", total: 5, auto: true, base: base + 2_000_001 },
      { rule: "collision", total: 3, auto: false, base: base + 3_000_001 },
      { rule: "mojibake", total: 2, auto: false, base: base + 4_000_001 },
      { rule: "encoding", total: 2, auto: true, base: base + 5_000_001 },
      { rule: "length", total: 1, auto: false, base: base + 6_000_001 },
    ];
    rulesByJob.set(jobId, r);
  }
  return r;
}

const NAMES = ["Verão", "Fotografias de família", "Relatório anual", "Música", "Coração", "Ação de formação"];
function issue(def: RuleDef, i: number): IssueView_Serialize {
  const id = def.base + i;
  const word = NAMES[i % NAMES.length];
  const enc = new TextEncoder();
  let before = `${word} ${i + 1}.jpg`;
  let after = before;
  let beforeBytes: number[];
  let afterBytes: number[];
  if (def.rule === "nfc") {
    beforeBytes = Array.from(enc.encode(before.normalize("NFD")));
    afterBytes = Array.from(enc.encode(before.normalize("NFC")));
  } else if (def.rule === "forbidden-chars") {
    before = `Notas: ${i + 1}?.txt`;
    after = before.replace(/[:?]/g, "_");
    beforeBytes = Array.from(enc.encode(before));
    afterBytes = Array.from(enc.encode(after));
  } else if (def.rule === "mojibake") {
    after = `${word}.pdf`;
    // The UTF-8 bytes read as Latin-1: "ã" (c3 a3) shows as "Ã£".
    before = Array.from(enc.encode(after), (b) => String.fromCharCode(b)).join("");
    beforeBytes = Array.from(enc.encode(before));
    afterBytes = Array.from(enc.encode(after));
  } else if (def.rule === "encoding") {
    after = `${word}.doc`;
    // Latin-1 bytes: every character below U+0100 is one byte.
    beforeBytes = Array.from(after, (c) => c.charCodeAt(0));
    before = after.replace(/[^\x20-\x7e]/g, "\ufffd");
    afterBytes = Array.from(enc.encode(after));
  } else if (def.rule === "length") {
    before = `${word} — relatório completo de todas as atividades do ano, versão final revista e aprovada.pdf`;
    after = `${before.slice(0, 30)}…${before.slice(-20)}`;
    beforeBytes = Array.from(enc.encode(before));
    afterBytes = Array.from(enc.encode(after));
  } else {
    before = `${word}.txt`;
    after = `${word} (2).txt`;
    beforeBytes = Array.from(enc.encode(before));
    afterBytes = Array.from(enc.encode(after));
  }
  const d = decisions.get(id);
  return {
    id,
    entry_idx: i + 1,
    path: `Fotos/${2010 + (i % 14)}/${before}`,
    rule: def.rule,
    severity: def.auto ? "auto" : "needs_decision",
    carried_over: false,
    message_key: `names.${def.rule}`,
    before: beforeBytes,
    after: afterBytes,
    before_text: before,
    after_text: after,
    decision: d?.decision ?? (def.auto ? "accepted" : "pending"),
    custom_name: d?.custom_name ?? null,
  };
}

/** Whether issue `i` of `def` matches a path filter (like the engine: a substring, ASCII case). */
function matches(def: RuleDef, i: number, query: string | null | undefined): boolean {
  if (!query) return true;
  return issue(def, i).path.toLowerCase().includes(query.trim().toLowerCase());
}

function ruleCounts(jobId: string, query: string | null = null): RuleCount[] {
  return rulesFor(jobId)
    .map((def) => {
      let pending = 0;
      let total = 0;
      for (let i = 0; i < def.total; i++) {
        if (query && !matches(def, i, query)) continue;
        total++;
        if (!def.auto && (decisions.get(def.base + i)?.decision ?? "pending") === "pending") pending++;
      }
      return { rule: def.rule, total, pending, no_proposal: 0 };
    })
    .filter((r) => r.total > 0);
}

function send(ev: Event_Serialize) {
  void emit("service-event", ev);
}

function setState(job: JobSummary_Serialize, to: JobState, error: string | null = null) {
  const from = job.state;
  job.state = to;
  job.updated_at = Date.now();
  send({ type: "job_state_changed", job_id: job.id, from, to, error });
  log(job.id, "state", { from, to, error });
  if (["completed", "completed_with_warnings", "cancelled", "failed"].includes(to)) {
    job.finished_at = Date.now();
    log(job.id, "finished", { state: to, warnings: 0, error });
    send({ type: "job_finished", job_id: job.id, state: to, error });
  }
}

let settings: AppSettings_Serialize = { bandwidth_limit: null, send_window: null };

/** Ms until the send window opens; 0 when it's open or there is none (the engine's rule). */
function windowOpensIn(): number {
  const w = settings.send_window;
  if (!w) return 0;
  const d = new Date();
  const now = d.getHours() * 60 + d.getMinutes();
  const { start_minute: s, end_minute: e } = w;
  const open = s < e ? now >= s && now < e : now >= s || now < e;
  if (open) return 0;
  return (((s - now + 1440) % 1440) * 60 - d.getSeconds()) * 1000;
}

/** Sends now, or waits for the window like the engine does. */
function sendOrWait(job: JobSummary_Serialize) {
  const wait = windowOpensIn();
  if (wait > 0) {
    setState(job, "waiting");
    send({ type: "send_window_wait", job_id: job.id, opens_at: Date.now() + wait });
    log(job.id, "send_window_wait", { opens_at: Date.now() + wait });
  } else {
    runTransfer(job);
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const timers = new Map<string, ReturnType<typeof setInterval>>();

/** Dummy files for the transfer queue: what is in flight on tick `n`, and what just arrived. */
const QUEUE_FILES: { dir: string; name: (i: number) => string; category: FileCategory; size: number; renamed?: (i: number) => string }[] = [
  { dir: "Fotos/2019/Verão", name: (i) => `IMG_${2000 + i}.jpg`, category: "image", size: 4_200_000 },
  { dir: "Fotos/2019/Verão", name: (i) => `VID_${i.toString().padStart(4, "0")}.mov`, category: "video", size: 2_300_000_000 },
  { dir: "Documentos", name: (i) => `Ata reunião ${i}.pdf`, category: "document", size: 210_000, renamed: (i) => `Ata reunia\u0303o ${i}.pdf` },
  { dir: "Documentos/Contratos", name: (i) => `contrato_${i}.docx`, category: "document", size: 48_000 },
  { dir: "Música", name: (i) => `faixa ${i}.flac`, category: "audio", size: 31_000_000 },
  { dir: "Projetos", name: (i) => `backup_${i}.zip`, category: "archive", size: 640_000_000 },
];

function queueAt(job: JobSummary_Serialize, n: number) {
  const slots = Math.max(1, Math.min(job.options.concurrency, 8));
  const in_flight: InFlightView_Serialize[] = Array.from({ length: slots }, (_, k) => {
    const f = QUEUE_FILES[k % QUEUE_FILES.length];
    const i = n + k;
    const part = ((n * 37 + k * 17) % 100) / 100;
    return { idx: i * 10 + k, name: f.name(i), dir: f.dir, category: f.category, renamed_from: f.renamed?.(i) ?? null, size: f.size, bytes: Math.round(f.size * part), bytes_per_sec: 2_000_000 + k * 1_500_000, eta_secs: Math.round((f.size * (1 - part)) / (2_000_000 + k * 1_500_000)) };
  });
  const entries: DoneView[] = Array.from({ length: 3 }, (_, k) => {
    const f = QUEUE_FILES[(n + k) % QUEUE_FILES.length];
    return { idx: n * 100 + k, name: f.name(n - 1 + k), category: f.category };
  });
  return { in_flight, entries };
}

/** A job's manifest, made up on demand from the entry's index so any size works. */
function entryAt(job: JobSummary_Serialize, idx: number): EntryView & { renamed: boolean } {
  const f = QUEUE_FILES[idx % QUEUE_FILES.length];
  const n = Math.floor(idx / QUEUE_FILES.length);
  const renamed = !!f.renamed && n % 3 === 0;
  const sent = idx < job.counts.done || ["completed", "completed_with_warnings"].includes(job.state);
  let status: EntryStatus = sent ? (job.counts.verified_ok ? "verified" : "sent") : "not_sent";
  if (sent && idx % 997 === 3) status = "failed";
  else if (sent && job.counts.verified_ok && idx % 1499 === 5) status = "different";
  else if (idx % 1777 === 11) status = "skipped";
  const size = Math.max(1_000, Math.round(f.size * (0.4 + ((idx * 7919) % 100) / 80)));
  return { idx, is_dir: false, name: f.name(n), dir: f.dir, source_name: renamed ? f.renamed!(n) : null, size, category: f.category, status, renamed };
}

const PROBLEMS: EntryStatus[] = ["different", "missing", "unverifiable", "failed"];

function matchingEntries(job: JobSummary_Serialize, filter: EntryFilter) {
  const q = filter.query?.trim().toLowerCase();
  const out: (EntryView & { renamed: boolean })[] = [];
  for (let i = 1; i <= Math.min(job.counts.files, 60_000); i++) {
    const e = entryAt(job, i);
    if (filter.statuses.length && !filter.statuses.includes(e.status)) continue;
    if (filter.categories.length && !filter.categories.includes(e.category)) continue;
    if (filter.renamed != null && e.renamed !== filter.renamed) continue;
    if (q && !`${e.dir}/${e.name}`.toLowerCase().includes(q)) continue;
    out.push(e);
  }
  const rank = (e: EntryView) => (PROBLEMS.includes(e.status) ? 0 : 1);
  if (filter.sort === "largest_first") out.sort((a, b) => b.size - a.size || a.idx - b.idx);
  if (filter.sort === "problems_first") out.sort((a, b) => rank(a) - rank(b) || a.idx - b.idx);
  return { out, rank };
}

/** A destination's folders, for the browser (docs/design/specs/D11). */
const FOLDERS: Record<string, string[]> = {
  "": ["Backup", "Documentos", "Fotos 2", "Fotos 10", "Fotos 2019", "Música"],
  Backup: ["2018", "2019", "2020"],
  "Backup/2018": [],
  "Backup/2019": ["Verão", "Natal"],
  "Backup/2019/Verão": [],
  "Backup/2019/Natal": [],
  "Backup/2020": [],
  Documentos: ["Contratos", "Recibos"],
  "Documentos/Contratos": [],
  "Documentos/Recibos": [],
  "Fotos 2": [],
  "Fotos 10": [],
  "Fotos 2019": [],
  Música: [],
};

/** The job groups, as the engine defines them (docs/design/specs/D06). */
const GROUPS: Record<JobGroup, JobState[]> = {
  active: ["analysing", "transferring", "retrying", "waiting", "verifying", "integrating", "paused"],
  needs_you: ["awaiting_approval", "failed"],
  not_sent: ["draft", "ready"],
  finished: ["completed", "completed_with_warnings"],
  cancelled: ["cancelled"],
};
/** The source re-check of each job (D10): when it ran, and whether it found changes. */
const rechecks = new Map<string, { at: number; changed: number }>();
const dismissedReclaim = new Set<string>();
function reclaimStatusOf(job: JobSummary_Serialize): ReclaimStatus_Serialize {
  const blockers: ReclaimStatus_Serialize["blockers"] = [];
  if (job.state !== "completed") blockers.push({ code: "not_completed", count: null });
  if (!job.options.verify) blockers.push({ code: "not_verified", count: null });
  const check = rechecks.get(job.id);
  if (!check) blockers.push({ code: "not_checked", count: null });
  else if (Date.now() - check.at > 24 * 3_600_000) blockers.push({ code: "stale_check", count: null });
  const sources = job.sources.map((path, i) => {
    const b: ReclaimStatus_Serialize["blockers"] = [];
    if (check?.changed && i === 0) b.push({ code: "changed", count: check.changed });
    const share = Math.round(job.counts.bytes / job.sources.length);
    return { path, files: Math.round(job.counts.files / job.sources.length), bytes: share, eligible: !b.length && !blockers.length, blockers: b };
  });
  const eligible = !blockers.length && sources.every((s) => s.eligible);
  return { job_id: job.id, eligible, bytes: sources.filter((s) => s.eligible).reduce((n, s) => n + s.bytes, 0), verified_at: job.finished_at, checked_at: check?.at ?? null, size_only: false, sources, blockers, changed_sample: check?.changed ? ["Documentos/Contratos/contrato_7.docx"] : [], dismissed: dismissedReclaim.has(job.id) };
}
function reclaimable(job: JobSummary_Serialize): boolean {
  const s = reclaimStatusOf(job);
  return s.eligible && !s.dismissed;
}

/** Each destination's space (bytes), and when it was last checked. */
const SPACE: Record<string, { total: number; used: number; quota?: { total: number; used: number } }> = {
  "p-nc": { total: 2_400_000_000_000, used: 1_588_000_000_000, quota: { total: 2_000_000_000_000, used: 1_210_000_000_000 } },
  "p-disk": { total: 2_000_000_000_000, used: 100_000_000_000 },
};
const checkedAt = new Map<string, number>([
  ["p-nc", now - 5 * 60_000],
  ["p-disk", now - 3 * 3_600_000],
]);
function spaceOf(id: string) {
  const s = SPACE[id] ?? { total: 1_000_000_000_000, used: 400_000_000_000 };
  return { total_bytes: s.total, used_bytes: s.used, free_bytes: s.total - s.used };
}
function storageOf(id: string): StorageView {
  const at = checkedAt.get(id) ?? null;
  const disk = spaceOf(id);
  const q = SPACE[id]?.quota;
  const quota = q ? { total_bytes: q.total, used_bytes: q.used, free_bytes: q.total - q.used } : null;
  const eff = quota && quota.free_bytes < disk.free_bytes ? quota : disk;
  const low = eff.free_bytes < 10e9 || eff.free_bytes < eff.total_bytes * 0.1;
  return { profile_id: id, ...eff, transport: disk, quota, checked_at: at, stale: at == null || Date.now() - at > 3_600_000, low, last_error: null };
}

function runTransfer(job: JobSummary_Serialize) {
  clearInterval(timers.get(job.id));
  const total = job.counts.bytes;
  let tick = 0;
  const t = setInterval(async () => {
    if (job.state !== "transferring") return;
    tick++;
    job.counts.bytes_done = Math.min(total, job.counts.bytes_done + total / 25);
    job.counts.done = Math.floor((job.counts.bytes_done / total) * job.counts.files);
    const q = queueAt(job, tick);
    send({ type: "transfer_progress", job_id: job.id, bytes_done: job.counts.bytes_done, bytes_total: total, files_done: job.counts.done, files_total: job.counts.files, bytes_per_sec: 48_000_000, eta_secs: Math.round((total - job.counts.bytes_done) / 48_000_000), in_flight: job.counts.bytes_done >= total ? [] : q.in_flight });
    send({ type: "entries_done", job_id: job.id, entries: q.entries, more: 37 });
    if (job.counts.bytes_done >= total) {
      clearInterval(t);
      timers.delete(job.id);
      log(job.id, "transfer_round", { sent: job.counts.files, failed: 0, bytes: total, first_error: null });
      if (job.options.verify) {
        setState(job, "verifying");
        for (let c = 0; c <= 4; c++) {
          await sleep(400);
          send({ type: "verify_progress", job_id: job.id, checked: Math.floor((job.counts.files * c) / 4), total: job.counts.files, mismatches: 0 });
        }
        job.counts.verified_ok = job.counts.files;
        log(job.id, "verify", { checked: job.counts.files, mismatch: 0, missing: 0, unverifiable: 0 });
      }
      if (profiles.find((p) => p.id === job.profile_id)?.integration === "nextcloud") {
        setState(job, "integrating");
        for (let c = 0; c <= 3; c++) {
          await sleep(500);
          send({ type: "integration_progress", job_id: job.id, step: "nextcloud.scan", done: c, total: 3, items: c * 400, current: `demo/files/Backup/Fotos/${2010 + c}`, waiting: null });
        }
      }
      job.counts.pending = 0;
      setState(job, "completed");
    }
  }, 500);
  timers.set(job.id, t);
}

function find(id: unknown): JobSummary_Serialize {
  const j = jobs.find((x) => x.id === id);
  if (!j) throw fail("not_found", `job ${String(id)} not found`);
  return j;
}

function fail(code: string, message: string): ErrorBody {
  return { code, message };
}

/** Refuses `op` where the engine would, with the engine's error codes. */
function allow(op: keyof typeof JOB_OPERATIONS, job: JobSummary_Serialize) {
  if ((JOB_OPERATIONS[op] as readonly JobState[]).includes(job.state)) return;
  if (op === "start" && job.state === "awaiting_approval") throw fail("pending_decisions", "name decisions are still pending");
  if (op === "set_options" || op === "decide" || op === "decide_all") throw fail("invalid", `${op === "set_options" ? "options" : "decisions"} can't change while the job is ${job.state}`);
  throw fail("invalid_transition", `cannot ${op} a job that is ${job.state}`);
}

const DECIDED: Record<DecisionInput["kind"], IssueDecision> = {
  accept: "accepted",
  edit: "edited",
  keep_original: "rejected",
  skip: "skipped",
};

function readyIfDecided(job: JobSummary_Serialize) {
  if (job.state === "awaiting_approval" && ruleCounts(job.id).every((r) => r.pending === 0)) setState(job, "ready");
}

type Commands = typeof commands;
/** What a command answers: the `data` of its result, or its plain return value. */
type Answer<K extends keyof Commands> = Awaited<ReturnType<Commands[K]>> extends infer R
  ? R extends { status: "ok"; data: infer D }
    ? D
    : R extends { status: "error" }
      ? never
      : R
  : never;
/** One handler per command in the bindings, answering that command's type. */
type Handlers = { [K in keyof Commands]: (a: Args) => Answer<K> | Promise<Answer<K>> };

// A small pretend disk for `inspect_sources`: a browser can't look at real paths.
type FakeNode = { dir?: number; capped?: boolean; link?: string; problem?: SourceProblem };
const FAKE_FS: Record<string, FakeNode> = {
  "/home/demo/Fotos": { dir: 412 },
  "/home/demo/Fotos/2023": { dir: 96 },
  "/home/demo/Documentos": { dir: 37 },
  "/home/demo/Música": { dir: SOURCE_CHILDREN_CAP, capped: true },
  "/home/demo/Atalho": { link: "/home/demo/Fotos" },
  "/home/demo/Privado": { dir: 0, problem: "unreadable" },
  "/dev/null": { problem: "special" },
};
const FAKE_GONE = ["/media/demo/disco-antigo"];
const EXT_CATEGORY: Record<string, FileCategory> = {
  jpg: "image", jpeg: "image", png: "image", heic: "image", mp4: "video", mov: "video", mp3: "audio", flac: "audio",
  pdf: "document", docx: "document", txt: "document", xlsx: "document", zip: "archive", tar: "archive",
};

/** "/a/./b/../c/" → "/a/c", like `canonicalize` without links. */
function normalPath(path: string): string {
  const out: string[] = [];
  for (const seg of path.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") out.pop();
    else out.push(seg);
  }
  return "/" + out.join("/");
}
const hash = (s: string) => [...s].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7);

function inspectOne(path: string): SourceInfo_Serialize {
  const info: SourceInfo_Serialize = { path, problem: null, is_dir: false, size: null, children: null, children_capped: false, mtime: null, category: null, covered_by: null, link_target: null };
  const p = normalPath(path);
  if (p === "/") return { ...info, problem: "root" };
  if (FAKE_GONE.some((g) => p === g || p.startsWith(g + "/"))) return { ...info, problem: "missing" };
  info.mtime = now - (hash(p) % 60) * 86_400_000;
  const node = FAKE_FS[p];
  if (node?.link) return { ...info, problem: "symlink", link_target: node.link };
  if (node?.problem) return { ...info, problem: node.problem, is_dir: node.dir != null };
  const name = p.slice(p.lastIndexOf("/") + 1);
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : null;
  if (node?.dir != null || !ext) {
    return { ...info, is_dir: true, children: node?.dir ?? hash(p) % 200, children_capped: node?.capped ?? false };
  }
  return { ...info, size: 10_000 + (hash(p) % 50_000_000), category: EXT_CATEGORY[ext] ?? "other" };
}

/** Like the engine: the outermost other source that contains each one, the earlier of duplicates. */
function inspectAll(paths: string[]): SourceInfo_Serialize[] {
  const out = paths.map(inspectOne);
  const canon = out.map((i) => (i.problem ? null : normalPath(i.path)));
  const first = new Map<string, number>();
  canon.forEach((c, i) => c && !first.has(c) && first.set(c, i));
  canon.forEach((c, i) => {
    if (!c) return;
    let outermost: number | null = null;
    for (let a = c; ; a = a.slice(0, a.lastIndexOf("/")) || "/") {
      const j = first.get(a);
      if (j != null && j !== i) outermost = j;
      if (a === "/") break;
    }
    out[i].covered_by = outermost;
  });
  return out;
}

const handlers: Handlers = {
  serviceEndpoint: () => "/run/user/1000/filebridge/service.sock (mock)",
  serviceReplacing: () => false,
  setTrayLabels: () => null,
  ping: () => ({ protocol: PROTOCOL_VERSION, version: "0.0.1-mock" }),
  listProfiles: () => profiles,
  saveProfile: (a) => {
    const p = { ...(a.profile as Profile_Serialize) };
    const added = !p.id;
    if (!p.id) p.id = newId("p");
    const i = profiles.findIndex((x) => x.id === p.id);
    if (i >= 0) profiles[i] = p;
    else profiles.push(p);
    if (added) appLog.push({ id: nextLogId++, at: Date.now(), kind: "profile", level: "info", data: { action: "added", profile_id: p.id, name: p.name }, jobId: null });
    return p;
  },
  deleteProfile: (a) => {
    // Like the service: not while a job to it could still be sent or resumed.
    const open = jobs.filter((j) => j.profile_id === a.id && !["completed", "completed_with_warnings", "cancelled", "failed"].includes(j.state));
    if (open.length) {
      throw fail("invalid", `${open.length} transfer(s) to this destination are not finished (${[...new Set(open.map((j) => j.state))].join(", ")}): cancel them, or let them finish, before deleting it`);
    }
    const i = profiles.findIndex((x) => x.id === a.id);
    if (i >= 0) profiles.splice(i, 1);
    return null;
  },
  testProfile: async (a) => {
    await sleep(600);
    const p = profiles.find((x) => x.id === a.id);
    const nc = p?.integration === "nextcloud";
    checkedAt.set(a.id as string, Date.now());
    return {
      transport: { source: "transport", ...spaceOf(a.id as string), writable: true, checks: [{ id: "connect", status: "ok", message: "connected" }, { id: "writable", status: "ok", message: "test file written and removed" }] },
      integration: nc ? { source: "integration", free_bytes: null, total_bytes: null, used_bytes: null, writable: true, checks: [{ id: "nextcloud.occ", status: "ok", message: "occ 30.0.2 answers" }, { id: "nextcloud.owner", status: "warn", message: "files will be chowned to www-data (33:33)" }] } : null,
    };
  },
  detectNextcloud: async (a) => {
    await sleep(1200);
    const req = a.req as {
      name: string;
      host: string;
      port: number;
      user: string;
      key_file: string | null;
      nextcloud_user: string;
      folder: string;
      ctid: number | null;
    };
    // A host named like Proxmox has `pct`; anything else is the Nextcloud machine itself.
    const proxmox = /proxmox|pve/i.test(req.host);
    const detector = proxmox ? "proxmox-lxc" : "host";
    const steps = proxmox
      ? [{ id: "pct-list", status: req.ctid == null ? "fail" : "ok", message: req.ctid == null ? "found 2 running containers named like Nextcloud; choose one" : `container ${req.ctid} (nextcloud) is running`, commands: ["pct list  (exit 0)"] }]
      : [
          { id: "pct-list", status: "ok", message: "no `pct` here, so this is not a Proxmox node: looking for Nextcloud on this machine itself", commands: ["pct list  (exit 127)", "test -x /usr/sbin/pct  (exit 1)"] },
          { id: "host", status: "ok", message: "not in a Proxmox container; commands run as root", commands: ["id -u  (exit 0)"] },
        ];
    if (proxmox && req.ctid == null) return { detector, steps, containers: ["105 nextcloud (running)", "112 nextcloud-test (running)"], profile: null };
    const run = (cmd: string) => (proxmox ? `pct exec ${req.ctid} -- ${cmd}` : cmd);
    const data = proxmox ? "/srv/ncdata" : "/mnt/ncdata";
    const files = `${data}/${req.nextcloud_user}/files`;
    steps.push(
      { id: "datadirectory", status: "ok", message: proxmox ? `/mnt/ncdata in the container is ${data} on the host` : data, commands: [`${run("grep -F datadirectory /var/www/nextcloud/config/config.php")}  (exit 0)`] },
      { id: "data-path", status: "ok", message: `${files} exists`, commands: [`test -d ${files}  (exit 0)`] },
      { id: "occ", status: "ok", message: "/var/www/nextcloud/occ, run as www-data", commands: [`${run("test -f /var/www/nextcloud/occ")}  (exit 0)`] },
      { id: "run-as", status: "ok", message: `Nextcloud 29.0.4 answers through su (no sudo ${proxmox ? "in the container" : "on this machine"})`, commands: [`${run("su -s /bin/bash www-data -c 'php /var/www/nextcloud/occ status --output=json'")}  (exit 0)`] },
      { id: "owner", status: "ok", message: proxmox ? "files will be owned by 100033:100033 (www-data 33:33 in the container)" : "files will be owned by 33:33 (www-data)", commands: [`stat -c %u:%g ${data}  (exit 0)`] },
    );
    // Built from the request, so it also works when the mock starts empty (VITE_MOCK_EMPTY).
    const folder = req.folder.split("/").filter(Boolean).join("/");
    const profile: Profile_Serialize = {
      id: newId("p"),
      name: req.name,
      destination: {
        transport: "rclone",
        backend: { kind: "sftp", host: req.host, port: req.port, user: req.user, key_file: req.key_file, known_hosts_file: null, password: null, key: null },
        root: folder ? `${files}/${folder}` : files,
      },
      integration: "nextcloud",
      integration_settings: { mode: "direct" },
      target: { family: "unix", case_sensitive: true, max_name_bytes: null, max_path_bytes: null },
    };
    profiles.push(profile);
    return { detector, steps, containers: [], profile };
  },
  createJob: (a) => {
    const id = newId("j");
    jobs.unshift({
      id,
      kind: "transfer",
      profile_id: a.profileId as string,
      state: "draft",
      error: null,
      attempt: 0,
      next_retry_at: null,
      created_at: Date.now(),
      updated_at: Date.now(),
      finished_at: null,
      sources: a.sources as string[],
      options: a.options as JobSummary_Serialize["options"],
      counts: counts(0, 0, 0),
      label: null,
      again_of: null,
    });
    log(id, "created", { sources: a.sources as string[], profile_id: a.profileId as string, options: a.options as Record<string, unknown> });
    return id;
  },
  addNextcloudWebdav: async (a) => {
    const req = a.req as NextcloudWebdavRequest;
    await sleep(600);
    const bare = req.server.replace(/^https?:\/\//, "");
    const host = bare.includes("/") ? bare.slice(0, bare.indexOf("/")) : bare;
    if (/^http:\/\//.test(req.server)) throw fail("invalid", "the address must start with https://: an app password must not travel unencrypted");
    if (host.includes("@")) throw fail("invalid", "leave the user and password out of the address: enter them in their own fields");
    // A pasted WebDAV address names the user's id, which an email login is not.
    const pastedId = /\/remote\.php\/dav\/files\/([^/]+)/.exec(bare)?.[1];
    const userId = pastedId ? decodeURIComponent(pastedId) : null;
    const url = `https://${host}/remote.php/dav/files/${encodeURIComponent(userId ?? req.user)}/`;
    const steps = [{ id: "address", status: "ok", message: `WebDAV address ${url}`, commands: [] as string[] }];
    // Demo: any app password shorter than 10 characters is refused, like a mistyped one.
    if (req.app_password.length < 10) {
      steps.push({ id: "sign-in", status: "fail", message: `Nextcloud refused ${req.user} with this app password (401 Unauthorized)`, commands: [] });
      return { detector: "webdav", steps, containers: [], profile: null };
    }
    steps.push({ id: "sign-in", status: "ok", message: `signed in as ${req.user}`, commands: [] });
    if (req.folder) steps.push({ id: "folder", status: "warn", message: `folder ${req.folder} doesn't exist yet: the first send creates it`, commands: [] });
    steps.push({ id: "space", status: "ok", message: "312.4 GB free of 1000.0 GB", commands: [] });
    const id = newId("p");
    const profile: Profile_Serialize = {
      id,
      name: req.name,
      destination: { transport: "rclone", backend: { kind: "webdav", url, user: req.user, password: `filebridge/${id}/app-password`, vendor: "nextcloud" }, root: req.folder },
      integration: "nextcloud",
      integration_settings: { mode: "webdav", server: `https://${host}`, user: req.user, ...(userId && userId !== req.user ? { user_id: userId } : {}) },
      target: { family: "unix", case_sensitive: true, max_name_bytes: null, max_path_bytes: null },
    };
    profiles.push(profile);
    return { detector: "webdav", steps, containers: [], profile };
  },
  installAppKey: async (a) => {
    const p = profiles.find((x) => x.id === a.profileId);
    if (!p) throw fail("not_found", `no profile ${a.profileId as string}`);
    const b = p.destination.backend;
    if (b.kind !== "sftp") throw fail("invalid", "only destinations reached over SSH (SFTP, Nextcloud direct mode) have an SSH key");
    if (b.key) throw fail("invalid", "this destination already uses filebridge's own key");
    if (b.password && !b.key_file)
      throw fail("invalid", "this destination signs in with a password: filebridge's own key is installed with a key file or SSH agent");
    if (!b.user) throw fail("invalid", "the profile names no SSH user");
    await sleep(900);
    const accounts = sshAccounts(p);
    b.key = `filebridge/${p.id}/ssh-key`;
    const step = (id: string, message: string) => ({ id, status: "ok", message, commands: [] as string[] });
    return {
      steps: [
        step("generate", "a new Ed25519 key, made on this computer"),
        ...accounts.map((who) => step("install", `added to ${who}'s authorized_keys, restricted`)),
        step("keyring", "the private key is in the system keyring"),
        ...accounts.map((who) => step("sign-in", `${who} signs in with the new key`)),
        step("transfer", "files can be sent with the new key"),
        step("saved", "the destination now uses filebridge's own key; your own key is no longer used"),
      ],
      profile: p,
    };
  },
  removeAccess: async (a) => {
    const p = profiles.find((x) => x.id === a.profileId);
    if (!p) throw fail("not_found", `no profile ${a.profileId as string}`);
    const b = p.destination.backend;
    if (b.kind !== "sftp" || !b.key) throw fail("invalid", "this destination doesn't use filebridge's own key");
    await sleep(900);
    const accounts = sshAccounts(p);
    b.key = null;
    const step = (id: string, message: string) => ({ id, status: "ok", message, commands: [] as string[] });
    return {
      steps: [
        ...accounts.map((who) => step("remove", `removed from ${who}'s authorized_keys`)),
        ...accounts.map((who) => step("proof", `${who} refuses the old key: access is gone`)),
        step(
          "saved",
          b.password ? "the destination signs in with its password again" : "the destination signs in with your key file or SSH agent again",
        ),
        step("keyring", "deleted from the system keyring"),
      ],
      profile: p,
    };
  },
  setProfilePassword: (a) => {
    const p = profiles.find((x) => x.id === a.profileId);
    if (!p) throw fail("not_found", `no profile ${a.profileId as string}`);
    if (p.destination.backend.kind === "local") throw fail("invalid", "a local folder has no password");
    // Like the service: only the reference is kept; the mock forgets the value.
    p.destination.backend.password = a.password ? `filebridge/${p.id}/app-password` : null;
    return p;
  },
  createRepairJob: (a) => {
    const id = newId("j");
    const dir = trimBoth((a.dir as string | null) ?? "", "/") || null;
    jobs.unshift({
      id,
      kind: "repair",
      profile_id: a.profileId as string,
      state: "draft",
      error: null,
      attempt: 0,
      next_retry_at: null,
      created_at: Date.now(),
      updated_at: Date.now(),
      finished_at: null,
      sources: [],
      options: { verify: false, concurrency: 8, target_dir: dir },
      counts: counts(0, 0, 0),
      label: null,
      again_of: null,
    });
    // What an old rsync leaves behind: garbled and NFD names, nothing that blocks.
    const base = (rulesByJob.size + 1) * 10_000_000;
    rulesByJob.set(id, [
      { rule: "mojibake", total: 37, auto: false, base: base + 1 },
      { rule: "nfc", total: 1_204, auto: true, base: base + 1_000_001 },
    ]);
    log(id, "created", { repair: dir ?? "", profile_id: a.profileId as string });
    return id;
  },
  setOptions: (a) => {
    const job = find(a.jobId);
    allow("set_options", job);
    if (job.kind === "repair") throw fail("invalid", "a repair's folder and options can't change; start a new repair instead");
    job.options = a.options as JobSummary_Serialize["options"];
    return null;
  },
  analyse: async (a) => {
    const job = find(a.jobId);
    allow("analyse", job);
    setState(job, "analysing");
    const files = 48_211;
    const bytes = 412_000_000_000;
    for (let s = 1; s <= 10; s++) {
      await sleep(250);
      const at = job.kind === "repair" ? (job.options.target_dir ?? "") : job.sources[0];
      send({ type: "scan_progress", job_id: job.id, files: Math.floor((files * s) / 10), bytes: Math.floor((bytes * s) / 10), current: `${at}/${2010 + s}` });
    }
    job.counts = counts(files, bytes, 0);
    const rules = ruleCounts(job.id);
    const pending = rules.reduce((n, r) => n + r.pending, 0);
    setState(job, pending ? "awaiting_approval" : "ready");
    log(job.id, "analysis", { files, bytes, scan_errors: 1, skipped_symlinks: 2 });
    checkedAt.set(job.profile_id, Date.now());
    const summary: AnalysisSummary_Serialize = {
      job_id: job.id,
      state: job.state,
      files,
      dirs: 1_204,
      bytes,
      rules,
      pending_decisions: pending,
      scan_errors: ["/home/demo/Fotos/privado: permission denied"],
      skipped_symlinks: 2,
      preflight: [{ source: "transport", ...spaceOf(job.profile_id), writable: true, checks: [{ id: "writable", status: "ok", message: "test file written and removed" }] }],
    };
    return summary;
  },
  ruleCounts: (a) => ruleCounts(a.jobId as string, a.query as string | null),
  plannedRenames: (a) => {
    const job = find(a.jobId);
    if (ruleCounts(job.id).some((r) => r.pending > 0)) throw fail("pending_decisions", "name decisions are still pending");
    let n = 0;
    for (const def of rulesFor(job.id)) {
      for (let i = 0; i < def.total; i++) {
        const d = def.auto ? "accepted" : (decisions.get(def.base + i)?.decision ?? "pending");
        if (d === "accepted" || d === "edited") n++;
      }
    }
    return n;
  },
  issues: (a) => {
    const defs = rulesFor(a.jobId as string).filter((d) => !a.rule || d.rule === a.rule);
    const after = (a.afterId as number | null) ?? 0;
    const out: IssueView_Serialize[] = [];
    for (const d of defs) {
      const start = Math.max(0, after - d.base + 1);
      for (let i = start; i < d.total && out.length < (a.limit as number); i++) if (matches(d, i, a.query as string | null)) out.push(issue(d, i));
    }
    return out;
  },
  decide: (a) => {
    const job = find(a.jobId);
    allow("decide", job);
    const d = a.decision as DecisionInput;
    decisions.set(a.issueId as number, { decision: DECIDED[d.kind], custom_name: d.kind === "edit" ? d.name : null });
    readyIfDecided(job);
    return null;
  },
  decideMany: (a) => {
    const d = a.decision as DecisionInput;
    if (d.kind === "edit") throw fail("invalid", "an edited name applies to one issue, not to many");
    const job = find(a.jobId);
    allow("decide", job);
    let changed = 0;
    for (const id of a.issueIds as number[]) {
      decisions.set(id, { decision: DECIDED[d.kind], custom_name: null });
      changed++;
    }
    readyIfDecided(job);
    return { changed, refused: [] };
  },
  decideAll: (a) => {
    const d = a.decision as DecisionInput;
    if (d.kind === "edit") throw fail("invalid", "an edited name applies to one issue, not to many");
    const job = find(a.jobId);
    allow("decide_all", job);
    let n = 0;
    for (const def of rulesFor(job.id)) {
      if (def.auto || (a.rule && def.rule !== a.rule)) continue;
      for (let i = 0; i < def.total; i++) {
        if ((decisions.get(def.base + i)?.decision ?? "pending") !== "pending") continue;
        if (!matches(def, i, a.query as string | null)) continue;
        decisions.set(def.base + i, { decision: DECIDED[d.kind], custom_name: null });
        n++;
      }
    }
    if (n) log(job.id, "decisions", { rule: (a.rule as string | null) ?? null, decision: DECIDED[d.kind], count: n });
    readyIfDecided(job);
    return n;
  },
  start: (a) => {
    const job = find(a.jobId);
    allow("start", job);
    setState(job, "transferring");
    sendOrWait(job);
    return null;
  },
  resume: (a) => handlers.start(a),
  pause: (a) => {
    const job = find(a.jobId);
    allow("pause", job);
    clearInterval(timers.get(job.id));
    setState(job, "paused");
    return null;
  },
  cancel: (a) => {
    const job = find(a.jobId);
    allow("cancel", job);
    clearInterval(timers.get(job.id));
    setState(job, "cancelled");
    return null;
  },
  listJobs: (a) => {
    const f = a.filter as JobFilter;
    const groups = new Set(f.groups.flatMap((g) => GROUPS[g]));
    const q = f.query?.trim().toLowerCase();
    let list = jobs.filter(
      (j) =>
        (!groups.size || groups.has(j.state)) &&
        (!f.profile_id || j.profile_id === f.profile_id) &&
        (!q || [j.label ?? "", ...j.sources, profiles.find((p) => p.id === j.profile_id)?.name ?? ""].some((t) => t.toLowerCase().includes(q))),
    );
    if (f.reclaimable) list = list.filter((j) => reclaimable(j));
    if (f.sort === "oldest") list = [...list].reverse();
    if (f.sort === "largest") list = [...list].sort((x, y) => y.counts.bytes - x.counts.bytes);
    const off = a.offset as number;
    return { items: list.slice(off, off + (a.limit as number)), total: list.length };
  },
  setLabel: (a) => {
    const job = find(a.jobId);
    const l = (a.label as string | null)?.trim() || null;
    if (l && l.length > 120) throw fail("invalid", "a name has at most 120 characters");
    job.label = l;
    return null;
  },
  sendAgain: (a) => {
    const old = find(a.jobId);
    if (!["completed", "completed_with_warnings", "cancelled", "failed"].includes(old.state)) throw fail("invalid", "only a finished transfer can do that");
    const id = newId("j");
    jobs.unshift({ ...structuredClone(old), id, state: "draft", error: null, attempt: 0, next_retry_at: null, created_at: Date.now(), updated_at: Date.now(), finished_at: null, counts: counts(0, 0, 0), profile_id: (a.profileId as string | null) ?? old.profile_id, again_of: old.id });
    log(id, "created", { sources: old.sources, profile_id: old.profile_id, options: old.options, again_of: old.id });
    return id;
  },
  job: (a) => find(a.jobId),
  entries: (a) => {
    const job = find(a.jobId);
    const filter = a.filter as EntryFilter;
    const { out, rank } = matchingEntries(job, filter);
    const after = a.after as { idx: number } | null;
    const start = after ? out.findIndex((e) => e.idx === after.idx) + 1 : 0;
    const limit = Math.min(a.limit as number, 500);
    const items = out.slice(start, start + limit);
    const last = items.at(-1);
    return {
      items: items.map(({ renamed: _r, ...e }) => e),
      next: items.length === limit && last ? { idx: last.idx, size: last.size, rank: rank(last) } : null,
    };
  },
  entryFacets: (a) => {
    const job = find(a.jobId);
    const { out } = matchingEntries(job, { statuses: [], categories: [], renamed: null, include_dirs: false, query: null, sort: "folder_order" });
    const by = new Map<EntryStatus, number>();
    const cats = new Map<string, { files: number; bytes: number }>();
    let renamed = 0;
    for (const e of out) {
      by.set(e.status, (by.get(e.status) ?? 0) + 1);
      const c = cats.get(e.category) ?? { files: 0, bytes: 0 };
      c.files++;
      c.bytes += e.size;
      cats.set(e.category, c);
      if (e.renamed) renamed++;
    }
    return {
      by_status: [...by].map(([status, files]) => ({ status, files })),
      by_category: [...cats].map(([category, c]) => ({ category: category as CategoryCount["category"], ...c })).sort((x, y) => y.bytes - x.bytes),
      renamed,
      problems: [...by].filter(([s]) => PROBLEMS.includes(s)).reduce((n, [, v]) => n + v, 0),
    };
  },
  entry: (a) => {
    const job = find(a.jobId);
    const { renamed, ...view } = entryAt(job, a.idx as number);
    const source = `${job.sources[0]}/${view.dir}/${view.source_name ?? view.name}`;
    const detail: EntryDetail_Serialize = {
      view,
      source_path: source,
      source_path_hex: null,
      dest_path: view.status === "not_sent" ? null : `/media/disco/Backup/${view.dir}/${view.name}`,
      mtime: 1_690_000_000_000 + (a.idx as number) * 37_000,
      hash: view.status === "verified" || view.status === "different" ? { algo: "sha256", hex: Array.from({ length: 32 }, (_, i) => (((a.idx as number) * 31 + i * 17) % 256).toString(16).padStart(2, "0")).join("") } : null,
      issues: renamed ? [issue(rulesFor(job.id)[0], a.idx as number)] : [],
      error: view.status === "failed" ? "permission denied" : null,
    };
    return detail;
  },
  browse: async (a) => {
    await sleep(250);
    const dir = trimBoth((a.dir as string | null) ?? "", "/");
    if (!(dir in FOLDERS)) throw fail("not_found", `${dir} not found`);
    const prefix = ((a.prefix as string | null) ?? "").toLowerCase();
    const folders = FOLDERS[dir]
      .filter((n) => !prefix || n.toLowerCase().startsWith(prefix))
      .map((name, i) => ({ name, path: dir ? `${dir}/${name}` : name, mtime: now - (i + 1) * 86_400_000 * 3 }));
    return { path: dir, segments: dir ? dir.split("/") : [], folders, truncated: false };
  },
  checkFolderName: (a) => {
    const name = a.name as string;
    const fixed = trimEnd(name.normalize("NFC").replace(/[<>:"\\|?*]/g, "_"), " .");
    return fixed === name && name ? { ok: true, proposed: null, message_key: null } : { ok: false, proposed: fixed || null, message_key: "names.forbidden-chars" };
  },
  profileStorage: async (a) => {
    if (a.refresh) {
      await sleep(600);
      checkedAt.set(a.id as string, Date.now());
    }
    return storageOf(a.id as string);
  },
  reclaimStatus: (a) => reclaimStatusOf(find(a.jobId)),
  recheckSources: async (a) => {
    const job = find(a.jobId);
    const total = job.counts.files;
    for (let i = 1; i <= 4; i++) {
      await sleep(300);
      send({ type: "reclaim_check_progress", job_id: job.id, checked: Math.round((total * i) / 4), total });
    }
    rechecks.set(job.id, { at: Date.now(), changed: 0 });
    return reclaimStatusOf(job);
  },
  dismissReclaim: (a) => {
    if (a.dismissed) dismissedReclaim.add(a.jobId as string);
    else dismissedReclaim.delete(a.jobId as string);
    return null;
  },
  exportReport: async (a) => {
    const job = find(a.jobId);
    if (!["completed", "completed_with_warnings", "cancelled", "failed"].includes(job.state)) throw fail("invalid", "only a finished transfer can do that");
    const total = job.counts.files;
    for (let i = 1; i <= 5; i++) {
      await sleep(250);
      send({ type: "report_progress", job_id: job.id, rows: Math.round((total * i) / 5), total });
    }
    const req = a.req as { path: string };
    return { path: req.path, rows: total, bytes: total * 142 };
  },
  activity: (a) => {
    const all: (LogEntry & { jobId: string | null })[] = [...logs.entries()].flatMap(([jobId, list]) => list.map((e) => ({ ...e, jobId })));
    all.push(...appLog);
    all.sort((x, y) => x.id - y.id);
    const seen = new Set<string>();
    const last = new Map<string, string>();
    const out = [];
    for (const e of all) {
      const d = e.data as Record<string, unknown>;
      const key = `${e.jobId ?? ""}`;
      let keep = ["created", "retry", "send_window_wait", "finished", "profile"].includes(e.kind) || (e.kind === "state" && d.to === "transferring");
      if (keep && e.kind === "state") {
        keep = !seen.has(`${key}:start`);
        seen.add(`${key}:start`);
      }
      if (!keep) continue;
      if ((e.kind === "retry" || e.kind === "send_window_wait") && last.get(key) === e.kind) continue;
      last.set(key, e.kind);
      const job = e.jobId ? jobs.find((j) => j.id === e.jobId) : undefined;
      out.push({
        id: e.id,
        at: e.at,
        kind: e.kind,
        level: e.level,
        job_id: e.jobId,
        job_label: job ? (job.label ?? job.sources[0]?.split("/").pop() ?? "—") : null,
        profile_name: job ? (profiles.find((p) => p.id === job.profile_id)?.name ?? null) : ((d.name as string) ?? null),
        data: e.data,
      });
    }
    out.reverse();
    const before = a.beforeId as number | null;
    return out.filter((e) => before == null || e.id < before).slice(0, a.limit as number);
  },
  jobCategories: (a) => {
    const job = find(a.jobId);
    const { files, bytes } = job.counts;
    const split: [CategoryCount["category"], number, number][] = [
      ["image", 0.52, 0.62],
      ["video", 0.41, 0.01],
      ["document", 0.04, 0.3],
      ["archive", 0.025, 0.002],
      ["other", 0.005, 0.068],
    ];
    return split
      .map(([category, b, f]) => ({ category, bytes: Math.round(bytes * b), files: Math.round(files * f) }))
      .filter((c) => c.files > 0);
  },
  inspectSources: async (a) => {
    await sleep(250);
    const paths = a.paths as string[];
    if (paths.length > 10_000) throw fail("invalid", "at most 10000 sources at a time");
    return inspectAll(paths);
  },
  recentSources: (a) => {
    const sent = jobs.filter((j) => !["draft", "analysing", "awaiting_approval", "ready"].includes(j.state));
    const out = new Map<string, RecentSource>();
    for (const j of sent) {
      for (const path of j.sources) {
        if (!out.has(path)) out.set(path, { path, last_job_id: j.id, last_profile_id: j.profile_id, last_sent_at: j.created_at, exists: true });
      }
    }
    // An unplugged disk, to show a source that is no longer there.
    out.set("/media/demo/disco-antigo/Scans", { path: "/media/demo/disco-antigo/Scans", last_job_id: "j-old", last_profile_id: "p-disk", last_sent_at: now - 20 * 86_400_000, exists: false });
    return [...out.values()].slice(0, a.limit as number);
  },
  jobLog: (a) => {
    find(a.jobId);
    const list = logs.get(a.jobId as string) ?? [];
    const limit = a.limit as number;
    if (a.afterId != null) return list.filter((e) => e.id > (a.afterId as number)).slice(0, limit);
    return list.filter((e) => a.beforeId == null || e.id < (a.beforeId as number)).reverse().slice(0, limit);
  },
  getSettings: () => settings,
  saveSettings: (a) => {
    const s = a.settings as AppSettings_Serialize;
    const w = s.send_window;
    if (w && w.start_minute === w.end_minute) throw fail("invalid", "a send window must start and end at different times");
    if (s.bandwidth_limit != null && s.bandwidth_limit < 1024) throw fail("invalid", "the bandwidth limit must be at least 1024 bytes per second");
    settings = { bandwidth_limit: s.bandwidth_limit ?? null, send_window: w ?? null };
    // Like the engine: waiting jobs go when the window opens, running ones wait when it closes.
    for (const job of jobs) {
      if (job.state === "waiting" && windowOpensIn() === 0) {
        setState(job, "transferring");
        runTransfer(job);
      } else if (job.state === "transferring" && windowOpensIn() > 0) {
        clearInterval(timers.get(job.id));
        sendOrWait(job);
      }
    }
    return null;
  },
};

/** `list_profiles` (what Tauri invokes) → `listProfiles` (the bindings' name). */
const camel = (cmd: string) => cmd.replace(/_(.)/g, (_, c: string) => c.toUpperCase());

async function handle(cmd: string, a: Args): Promise<unknown> {
  const handler = (handlers as Record<string, ((a: Args) => unknown) | undefined>)[camel(cmd)];
  if (handler) return handler(a);
  if (cmd.startsWith("plugin:")) return null;
  throw fail("unsupported", `mock: ${cmd}`);
}

/** Asks for a path in a page dialog, with an example: a browser can't see real paths. */
function pickInPage(opts: PickOptions): Promise<string[]> {
  const example = opts.directory ? "/home/demo/Fotos" : "/home/demo/Documentos/relatório.pdf";
  const dialog = document.createElement("dialog");
  dialog.className = "card";
  const form = document.createElement("form");
  form.method = "dialog";
  const label = document.createElement("label");
  label.textContent = `Demo mode: type the ${opts.directory ? "folder" : "file"} path (the real app opens the system picker)`;
  const input = document.createElement("input");
  input.value = example;
  input.style.cssText = "display:block;width:100%;margin:.5rem 0";
  label.append(input);
  // Not a submit button, so Enter in the field means OK.
  const cancel = document.createElement("button");
  cancel.type = "button";
  cancel.className = "btn";
  cancel.textContent = "Cancel";
  cancel.addEventListener("click", () => dialog.close("cancel"));
  const ok = document.createElement("button");
  ok.className = "btn btn-primary";
  ok.value = "ok";
  ok.textContent = "OK";
  form.append(label, cancel, " ", ok);
  dialog.append(form);
  document.body.append(dialog);
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => {
      const path = dialog.returnValue === "ok" ? input.value.trim() : "";
      dialog.remove();
      resolve(path ? [path] : []);
    });
    dialog.showModal();
    input.select();
  });
}

export function installMock() {
  setMockPicker(pickInPage);
  // "Save as": a fixed folder in the demo home, under the suggested name.
  setMockSave(async (name) => `/home/demo/Relatórios/${name}`);
  mockWindows("main");
  // Cloned, like real IPC answers: the UI must never share objects with the fake service.
  mockIPC(async (cmd, payload) => structuredClone(await handle(cmd, (payload ?? {}) as Args)), { shouldMockEvents: true });
}
