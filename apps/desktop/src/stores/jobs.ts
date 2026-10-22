// Jobs and their live progress. Subscribes to the service's events once and keeps a normalised
// map by job id (docs/06). Summaries come from the service; `live` holds what only events carry
// (speed, ETA, current file, retries, the send window, integration step).
import { defineStore } from "pinia";
import { ref } from "vue";
import type { DoneView, Event_Serialize, JobState, JobSummary_Serialize } from "@/bindings";
import { api, errorMessage, events } from "@/lib/api";
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
  /** When the send window opens, while the job waits for it. */
  window?: Of<"send_window_wait">;
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

  async function loadPage(newOffset = offset.value) {
    try {
      // One extra row tells whether there is a next page.
      const rows = await api.listJobs(PAGE_SIZE + 1, newOffset);
      hasMore.value = rows.length > PAGE_SIZE;
      const shown = rows.slice(0, PAGE_SIZE);
      for (const j of shown) byId.value[j.id] = j;
      page.value = shown.map((j) => j.id);
      offset.value = newOffset;
      loadError.value = null;
    } catch (e) {
      loadError.value = errorMessage(e);
    }
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
    const where = profile?.name ?? "";
    const ok = ev.state === "completed" || ev.state === "completed_with_warnings";
    if (ev.state === "cancelled") return;
    await notify(
      ok ? t("notify.doneTitle") : t("notify.failedTitle"),
      ok
        ? t(ev.state === "completed" ? "notify.doneBody" : "notify.warningsBody", { where })
        : t("notify.failedBody", { where, error: ev.error ?? "" }),
    );
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

  return { byId, live, page, arrivedLastMinute, offset, hasMore, loadError, liveOf, init, loadPage, refresh, pause, resume, cancel };
});
