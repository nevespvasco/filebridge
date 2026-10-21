// Development only: `VITE_MOCK=1 pnpm dev` runs the UI in a plain browser against this in-memory
// fake of filebridge-service (through Tauri's `mockIPC`). It is never bundled into the app: main.ts
// imports it only when VITE_MOCK is set. Data shapes come from the generated bindings.
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type {
  AnalysisSummary_Serialize,
  DecisionInput,
  Event_Serialize,
  IssueView_Serialize,
  JobState,
  JobSummary_Serialize,
  Profile_Serialize,
  RuleCount,
} from "@/bindings";

type Args = Record<string, unknown>;

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
    options: { verify: true, concurrency: 8, bwlimit: null },
    counts: { ...counts(1234, 4_500_000_000, 1), verified_ok: 1234 },
  },
];

// Issues are generated on demand, so a rule can have hundreds of thousands of them.
type RuleDef = { rule: string; total: number; auto: boolean; base: number };
const rulesByJob = new Map<string, RuleDef[]>();
const decisions = new Map<number, { decision: string; custom_name: string | null }>();

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
  if (["completed", "completed_with_warnings", "cancelled", "failed"].includes(to)) {
    job.finished_at = Date.now();
    send({ type: "job_finished", job_id: job.id, state: to, error });
  }
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const timers = new Map<string, ReturnType<typeof setInterval>>();

function runTransfer(job: JobSummary_Serialize) {
  clearInterval(timers.get(job.id));
  const total = job.counts.bytes;
  const t = setInterval(async () => {
    if (job.state !== "transferring") return;
    job.counts.bytes_done = Math.min(total, job.counts.bytes_done + total / 25);
    job.counts.done = Math.floor((job.counts.bytes_done / total) * job.counts.files);
    send({ type: "transfer_progress", job_id: job.id, bytes_done: job.counts.bytes_done, bytes_total: total, files_done: job.counts.done, files_total: job.counts.files, bytes_per_sec: 48_000_000, eta_secs: Math.round((total - job.counts.bytes_done) / 48_000_000) });
    if (job.counts.bytes_done >= total) {
      clearInterval(t);
      timers.delete(job.id);
      if (job.options.verify) {
        setState(job, "verifying");
        for (let c = 0; c <= 4; c++) {
          await sleep(400);
          send({ type: "verify_progress", job_id: job.id, checked: Math.floor((job.counts.files * c) / 4), total: job.counts.files, mismatches: 0 });
        }
        job.counts.verified_ok = job.counts.files;
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
  if (!j) throw { code: "not_found", message: `job ${String(id)} not found` };
  return j;
}

async function handle(cmd: string, a: Args): Promise<unknown> {
  switch (cmd) {
    case "service_endpoint":
      return "/run/user/1000/filebridge/service.sock (mock)";
    case "set_tray_labels":
      return null;
    case "ping":
      return { protocol: 1, version: "0.0.1-mock" };
    case "list_profiles":
      return profiles;
    case "save_profile": {
      const p = { ...(a.profile as Profile_Serialize) };
      if (!p.id) p.id = `p-${Math.random().toString(36).slice(2, 8)}`;
      const i = profiles.findIndex((x) => x.id === p.id);
      if (i >= 0) profiles[i] = p;
      else profiles.push(p);
      return p;
    }
    case "delete_profile": {
      const i = profiles.findIndex((x) => x.id === a.id);
      if (i >= 0) profiles.splice(i, 1);
      return null;
    }
    case "test_profile": {
      await sleep(600);
      const p = profiles.find((x) => x.id === a.id);
      const nc = p?.integration === "nextcloud";
      return {
        transport: { source: "transport", free_bytes: nc ? 812_000_000_000 : 1_900_000_000_000, writable: true, checks: [{ id: "connect", status: "ok", message: "connected" }, { id: "writable", status: "ok", message: "test file written and removed" }] },
        integration: nc ? { source: "integration", free_bytes: null, writable: true, checks: [{ id: "nextcloud.occ", status: "ok", message: "occ 30.0.2 answers" }, { id: "nextcloud.owner", status: "warn", message: "files will be chowned to www-data (33:33)" }] } : null,
      };
    }
    case "detect_nextcloud": {
      await sleep(1200);
      const req = a.req as { name: string; ctid: number | null };
      const steps = [
        { id: "ssh", status: "ok", message: "connected as root", commands: ["ssh -p 22 root@pve.example.lan true"] },
        { id: "pct-list", status: req.ctid == null ? "warn" : "ok", message: req.ctid == null ? "2 containers look like Nextcloud" : `container ${req.ctid}`, commands: ["pct list"] },
      ];
      if (req.ctid == null) return { steps, containers: ["105 nextcloud", "112 nextcloud-test"], profile: null };
      steps.push(
        { id: "datadirectory", status: "ok", message: "/mnt/ncdata", commands: [`pct exec ${req.ctid} -- sudo -u www-data php /var/www/nextcloud/occ config:system:get datadirectory`] },
        { id: "data-path", status: "ok", message: "/srv/ncdata exists", commands: [`pct config ${req.ctid}`] },
        { id: "occ", status: "ok", message: "/var/www/nextcloud/occ, run as www-data", commands: [] },
        { id: "owner", status: "ok", message: "100033:100033", commands: ["stat -c %u:%g /srv/ncdata/demo/files"] },
      );
      const profile: Profile_Serialize = { ...profiles[0], id: `p-${Math.random().toString(36).slice(2, 8)}`, name: req.name };
      profiles.push(profile);
      return { steps, containers: [], profile };
    }
    case "create_job": {
      const id = `j-${Math.random().toString(36).slice(2, 8)}`;
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
      return id;
    }
    case "analyse": {
      const job = find(a.jobId);
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
    }
    case "rule_counts":
      return ruleCounts(a.jobId as string);
    case "issues": {
      const defs = rulesFor(a.jobId as string).filter((d) => !a.rule || d.rule === a.rule);
      const after = (a.afterId as number | null) ?? 0;
      const out: IssueView_Serialize[] = [];
      for (const d of defs) {
        const start = Math.max(0, after - d.base + 1);
        for (let i = start; i < d.total && out.length < (a.limit as number); i++) out.push(issue(d, i));
      }
      return out;
    }
    case "decide": {
      const d = a.decision as DecisionInput;
      const map = { accept: "accepted", edit: "edited", keep_original: "rejected", skip: "skipped" } as const;
      decisions.set(a.issueId as number, { decision: map[d.kind], custom_name: d.kind === "edit" ? d.name : null });
      const job = find(a.jobId);
      if (job.state === "awaiting_approval" && ruleCounts(job.id).every((r) => r.pending === 0)) setState(job, "ready");
      return null;
    }
    case "start":
    case "resume": {
      const job = find(a.jobId);
      if (job.state === "awaiting_approval") throw { code: "pending_decisions", message: "name decisions are still pending" };
      setState(job, "transferring");
      runTransfer(job);
      return null;
    }
    case "pause": {
      const job = find(a.jobId);
      clearInterval(timers.get(job.id));
      setState(job, "paused");
      return null;
    }
    case "cancel": {
      const job = find(a.jobId);
      clearInterval(timers.get(job.id));
      setState(job, "cancelled");
      return null;
    }
    case "list_jobs":
      return jobs.slice(a.offset as number, (a.offset as number) + (a.limit as number));
    case "job":
      return find(a.jobId);
    default:
      if (cmd.startsWith("plugin:")) return null;
      throw { code: "unsupported", message: `mock: ${cmd}` };
  }
}

export function installMock() {
  mockWindows("main");
  // Cloned, like real IPC answers: the UI must never share objects with the fake service.
  mockIPC(async (cmd, payload) => structuredClone(await handle(cmd, (payload ?? {}) as Args)), { shouldMockEvents: true });
}
