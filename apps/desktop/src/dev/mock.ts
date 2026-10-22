// Development only: `VITE_MOCK=1 pnpm dev` runs the UI in a plain browser against this in-memory
// fake of filebridge-service (through Tauri's `mockIPC`), for visual work. It is never bundled into
// the app: main.ts imports it only when VITE_MOCK is set. For real behaviour without rclone, run
// the app against `filebridge-service --fake-transport` instead (see README).
//
// It can't drift from the service: every command in the generated bindings needs a handler here
// with the command's own answer type (`Handlers`), and which operation is allowed in which job
// state comes from the engine's table (`JOB_OPERATIONS`), not from a copy.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { setMockPicker, type PickOptions } from "@/lib/platform";
import { emit } from "@tauri-apps/api/event";
import {
  JOB_OPERATIONS,
  PROTOCOL_VERSION,
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
  type JobSummary_Serialize,
  type LogEntry,
  type Profile_Serialize,
  type RuleCount,
} from "@/bindings";

type Args = Record<string, unknown>;

/** A short random id, like the service's (`p-…` for profiles, `j-…` for jobs). */
const newId = (prefix: string) => `${prefix}-${crypto.randomUUID().slice(0, 8)}`;

const now = Date.now();
const profiles: Profile_Serialize[] = [
  {
    id: "p-nc",
    name: "Nextcloud casa — demo/Backup",
    destination: { transport: "rclone", backend: { kind: "sftp", host: "pve.example.lan", port: null, user: "root", key_file: null, known_hosts_file: null, password: null }, root: "/srv/nextcloud/data/demo/files/Backup" },
    integration: "nextcloud",
    integration_settings: { mode: "direct" },
    target: { family: "unix", case_sensitive: true },
  },
  {
    id: "p-disk",
    name: "Disco externo",
    destination: { transport: "rclone", backend: { kind: "local" }, root: "/media/disco/Backup" },
    integration: null,
    integration_settings: null,
    target: { family: "windows", case_sensitive: false },
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
    options: { verify: true, concurrency: 8 },
    counts: { ...counts(1234, 4_500_000_000, 1), verified_ok: 1234 },
  },
];

// `VITE_MOCK_EMPTY=1`: start like a new user, with no destinations and no jobs.
if (import.meta.env.VITE_MOCK_EMPTY === "1") {
  profiles.length = 0;
  jobs.length = 0;
}

// The job log, as the engine records it (docs/design/specs/D07): parameters only.
const logs = new Map<string, LogEntry[]>();
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
    message_key: `names.${def.rule}`,
    before: beforeBytes,
    after: afterBytes,
    before_text: before,
    after_text: after,
    decision: d?.decision ?? (def.auto ? "accepted" : "pending"),
    custom_name: d?.custom_name ?? null,
  };
}

function ruleCounts(jobId: string): RuleCount[] {
  return rulesFor(jobId).map((def) => {
    let pending = 0;
    if (!def.auto) for (let i = 0; i < def.total; i++) if ((decisions.get(def.base + i)?.decision ?? "pending") === "pending") pending++;
    return { rule: def.rule, total: def.total, pending };
  });
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

const handlers: Handlers = {
  serviceEndpoint: () => "/run/user/1000/filebridge/service.sock (mock)",
  setTrayLabels: () => null,
  ping: () => ({ protocol: PROTOCOL_VERSION, version: "0.0.1-mock" }),
  listProfiles: () => profiles,
  saveProfile: (a) => {
    const p = { ...(a.profile as Profile_Serialize) };
    if (!p.id) p.id = newId("p");
    const i = profiles.findIndex((x) => x.id === p.id);
    if (i >= 0) profiles[i] = p;
    else profiles.push(p);
    return p;
  },
  deleteProfile: (a) => {
    const i = profiles.findIndex((x) => x.id === a.id);
    if (i >= 0) profiles.splice(i, 1);
    return null;
  },
  testProfile: async (a) => {
    await sleep(600);
    const p = profiles.find((x) => x.id === a.id);
    const nc = p?.integration === "nextcloud";
    return {
      transport: { source: "transport", free_bytes: nc ? 812_000_000_000 : 1_900_000_000_000, writable: true, checks: [{ id: "connect", status: "ok", message: "connected" }, { id: "writable", status: "ok", message: "test file written and removed" }] },
      integration: nc ? { source: "integration", free_bytes: null, writable: true, checks: [{ id: "nextcloud.occ", status: "ok", message: "occ 30.0.2 answers" }, { id: "nextcloud.owner", status: "warn", message: "files will be chowned to www-data (33:33)" }] } : null,
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
        backend: { kind: "sftp", host: req.host, port: req.port, user: req.user, key_file: req.key_file, known_hosts_file: null, password: null },
        root: folder ? `${files}/${folder}` : files,
      },
      integration: "nextcloud",
      integration_settings: { mode: "direct" },
      target: { family: "unix", case_sensitive: true },
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
    });
    log(id, "created", { sources: a.sources as string[], profile_id: a.profileId as string, options: a.options as Record<string, unknown> });
    return id;
  },
  setOptions: (a) => {
    const job = find(a.jobId);
    allow("set_options", job);
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
      send({ type: "scan_progress", job_id: job.id, files: Math.floor((files * s) / 10), bytes: Math.floor((bytes * s) / 10), current: `${job.sources[0]}/${2010 + s}` });
    }
    job.counts = counts(files, bytes, 0);
    const rules = ruleCounts(job.id);
    const pending = rules.reduce((n, r) => n + r.pending, 0);
    setState(job, pending ? "awaiting_approval" : "ready");
    log(job.id, "analysis", { files, bytes, scan_errors: 1, skipped_symlinks: 2 });
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
      preflight: [{ source: "transport", free_bytes: 812_000_000_000, writable: true, checks: [{ id: "writable", status: "ok", message: "test file written and removed" }] }],
    };
    return summary;
  },
  ruleCounts: (a) => ruleCounts(a.jobId as string),
  issues: (a) => {
    const defs = rulesFor(a.jobId as string).filter((d) => !a.rule || d.rule === a.rule);
    const after = (a.afterId as number | null) ?? 0;
    const out: IssueView_Serialize[] = [];
    for (const d of defs) {
      const start = Math.max(0, after - d.base + 1);
      for (let i = start; i < d.total && out.length < (a.limit as number); i++) out.push(issue(d, i));
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
  listJobs: (a) => jobs.slice(a.offset as number, (a.offset as number) + (a.limit as number)),
  job: (a) => find(a.jobId),
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
  mockWindows("main");
  // Cloned, like real IPC answers: the UI must never share objects with the fake service.
  mockIPC(async (cmd, payload) => structuredClone(await handle(cmd, (payload ?? {}) as Args)), { shouldMockEvents: true });
}
