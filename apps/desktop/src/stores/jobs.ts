// Jobs and their live progress. Subscribes to the service's events once and keeps a normalised
// map by job id (docs/06). Summaries come from the service; `live` holds what only events carry
// (speed, ETA, current file, retries, the send window, integration step).
import { defineStore } from "pinia";
import { ref } from "vue";
import type { DoneView, Event_Serialize, JobFilter, JobState, JobSummary_Serialize } from "@/bindings";
import { api, errorMessage, events } from "@/lib/api";
import { jobLabel } from "@/lib/jobs";
import { notify } from "@/lib/platform";
import { i18n } from "@/i18n";
import { useConnection } from "./connection";
import { useProfiles } from "./profiles";
import { useSettings } from "./settings";

type Of<T extends Event_Serialize["type"]> = Extract<Event_Serialize, { type: T }>;

export type Live = {
  scan?: Of<"scan_progress">;
  transfer?: Of<"transfer_progress">;
  verify?: Of<"verify_progress">;
  integration?: Of<"integration_progress">;
  retry?: Of<"retry_scheduled"> & { at: number };
  /** Files of the sources re-checked so far, while a reclaim check runs. */
  reclaim?: Of<"reclaim_check_progress">;
  /** Rows of a report written so far, while one is being exported. */
  report?: Of<"report_progress">;
  /** When the send window opens, while the job waits for it. */
  window?: Of<"send_window_wait">;
  /** The destination's free space against what is left to send, measured during the transfer. */
  space?: Of<"destination_space">;
  /** The last files that arrived, newest first (the queue's "Just finished"). */
  arrived: DoneView[];
  /** Arrivals counted per event, for "+N in the last minute". */
  arrivals: { at: number; n: number }[];
  failures: Of<"entry_failed">[];
  problems: Of<"integration_problem">[];
};

const MAX_LINES = 500;
/** Files kept in "Just finished". */
const MAX_ARRIVED = 20;
const MINUTE = 60_000;
export const PAGE_SIZE = 50;

/** States in which the service is working on the job. */
export const RUNNING: JobState[] = ["transferring", "retrying", "waiting", "verifying", "integrating"];
export const TERMINAL: JobState[] = ["completed", "completed_with_warnings", "cancelled", "failed"];

/** Retries after which a retrying job needs the user: the backoff (30 s … 15 min) adds up to
 *  about an hour by the 8th attempt. */
const LONG_RETRY_ATTEMPTS = 8;

/** Whether a job is waiting for the user: decisions to make, a failure, or retrying for long. */
export function needsYou(job: Pick<JobSummary_Serialize, "state" | "attempt">): boolean {
  return (
    job.state === "awaiting_approval" ||
    job.state === "failed" ||
    (job.state === "retrying" && job.attempt >= LONG_RETRY_ATTEMPTS)
  );
}

export const useJobs = defineStore("jobs", () => {
  const byId = ref<Record<string, JobSummary_Serialize>>({});
  const live = ref<Record<string, Live>>({});
  /** Ids on the current Jobs page, newest first. */
  const page = ref<string[]>([]);
  const offset = ref(0);
  const hasMore = ref(false);
  const loadError = ref<string | null>(null);
  let subscribed = false;
  const refreshTimers = new Map<string, ReturnType<typeof setTimeout>>();

  function liveOf(id: string): Live {
    return (live.value[id] ??= { arrived: [], arrivals: [], failures: [], problems: [] });
  }

  function push<T>(list: T[], item: T) {
    list.push(item);
    if (list.length > MAX_LINES) list.splice(0, list.length - MAX_LINES);
  }

  /** The Transfers list's filter (docs/design/specs/D06). */
  const filter = ref<JobFilter>({ query: null, groups: [], profile_id: null, reclaimable: false, sort: "newest" });
  const total = ref(0);

  async function loadPage(newOffset = offset.value) {
    try {
      const res = await api.listJobs(filter.value, PAGE_SIZE, newOffset);
      for (const j of res.items) byId.value[j.id] = j;
      page.value = res.items.map((j) => j.id);
      total.value = res.total;
      hasMore.value = newOffset + res.items.length < res.total;
      offset.value = newOffset;
      loadError.value = null;
    } catch (e) {
      loadError.value = errorMessage(e);
    }
  }

  async function setLabel(id: string, label: string | null) {
    await api.setLabel(id, label);
    await refresh(id);
  }

  /** A new draft from a finished job; returns it. */
  async function sendAgain(id: string, profileId: string | null): Promise<JobSummary_Serialize | undefined> {
    const newId = await api.sendAgain(id, profileId);
    return refresh(newId);
  }

  async function refresh(id: string): Promise<JobSummary_Serialize | undefined> {
    try {
      const j = await api.job(id);
      byId.value[id] = j;
      return j;
    } catch {
      return undefined;
    }
  }

  /** Coalesces the refreshes a burst of events would cause. */
  function refreshSoon(id: string) {
    if (refreshTimers.has(id)) return;
    refreshTimers.set(
      id,
      setTimeout(() => {
        refreshTimers.delete(id);
        void refresh(id);
      }, 400),
    );
  }

  function onEvent(ev: Event_Serialize) {
    const id = ev.job_id;
    const l = liveOf(id);
    const now = Date.now();
    switch (ev.type) {
      case "scan_progress":
        l.scan = ev;
        break;
      case "transfer_progress":
        l.transfer = ev;
        break;
      case "reclaim_check_progress":
        l.reclaim = ev;
        break;
      case "report_progress":
        l.report = ev;
        break;
      case "entries_done":
        l.arrived = [...[...ev.entries].reverse(), ...l.arrived].slice(0, MAX_ARRIVED);
        l.arrivals = [...l.arrivals.filter((a) => now - a.at < MINUTE), { at: now, n: ev.entries.length + ev.more }];
        break;
      case "verify_progress":
        l.verify = ev;
        break;
      case "integration_progress":
        l.integration = ev;
        break;
      case "retry_scheduled":
        l.retry = { ...ev, at: now };
        refreshSoon(id);
        break;
      case "send_window_wait":
        l.window = ev;
        refreshSoon(id);
        break;
      case "destination_space":
        l.space = ev;
        break;
      case "entry_failed":
        push(l.failures, ev);
        break;
      case "integration_problem":
        push(l.problems, ev);
        break;
      case "job_state_changed": {
        const j = byId.value[id];
        if (j) j.state = ev.to;
        if (ev.to !== "retrying") l.retry = undefined;
        if (ev.to !== "waiting") l.window = undefined;
        if (!RUNNING.includes(ev.to)) l.space = undefined;
        // Paused with an error: the engine paused it (a full destination), not the user.
        if (ev.to === "paused" && ev.error) void pausedByEngine(id);
        refreshSoon(id);
        if (!j) void loadPage(0);
        break;
      }
      case "job_finished":
        void finished(ev);
        break;
    }
  }

  async function finished(ev: Of<"job_finished">) {
    const job = await refresh(ev.job_id);
    if (!useSettings().notifications) return;
    const t = i18n.global.t;
    const profile = job ? useProfiles().byId[job.profile_id] : undefined;
    // The transfer's name leads, so two sends to one place tell apart.
    const where = [job ? jobLabel(job) : "", profile?.name].filter(Boolean).join(" → ");
    const ok = ev.state === "completed" || ev.state === "completed_with_warnings";
    if (ev.state === "cancelled") return;
    await notify(
      ok ? t("notify.doneTitle") : t("notify.failedTitle"),
      ok
        ? t(ev.state === "completed" ? "notify.doneBody" : "notify.warningsBody", { where })
        : t("notify.failedBody", { where, error: ev.error ?? "" }),
    );
  }

  async function pausedByEngine(id: string) {
    const job = await refresh(id);
    if (!job || job.error_code !== "destination_full" || !useSettings().notifications) return;
    const t = i18n.global.t;
    const where = [jobLabel(job), useProfiles().byId[job.profile_id]?.name].filter(Boolean).join(" → ");
    await notify(t("storage.fullNotifyTitle"), t("storage.fullNotifyBody", { where }));
  }

  async function init() {
    if (subscribed) return;
    subscribed = true;
    await events.serviceEvent.listen((e) => onEvent(e.payload));
    useConnection().onReconnect(() => {
      void loadPage();
      for (const id of Object.keys(byId.value)) refreshSoon(id);
    });
  }

  async function pause(id: string) {
    await api.pause(id);
    await refresh(id);
  }
  async function resume(id: string) {
    await api.resume(id);
    await refresh(id);
  }
  async function cancel(id: string) {
    await api.cancel(id);
    await refresh(id);
  }

  /** Files that arrived in the last minute (from `entries_done`). */
  function arrivedLastMinute(id: string): number {
    const now = Date.now();
    return (live.value[id]?.arrivals ?? []).filter((a) => now - a.at < MINUTE).reduce((sum, a) => sum + a.n, 0);
  }

  return { byId, live, page, filter, total, setLabel, sendAgain, arrivedLastMinute, offset, hasMore, loadError, liveOf, init, loadPage, refresh, pause, resume, cancel };
});
