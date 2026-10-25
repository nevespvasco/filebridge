// What Home shows (docs/design/specs/D01-home.md): bounded queries of the transfers that need the
// user, the running ones and the last finished ones. Never the whole list.
import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import type { JobFilter, JobGroup, JobSummary_Serialize } from "@/bindings";
import { api } from "@/lib/api";
import { useConnection } from "./connection";
import { needsYou, useJobs } from "./jobs";

const SEEN_KEY = "filebridge.seen";

function filter(groups: JobGroup[]): JobFilter {
  return { query: null, groups, profile_id: null, reclaimable: false, sort: "newest" };
}

export const useHome = defineStore("home", () => {
  const jobs = useJobs();
  const attention = shallowRef<JobSummary_Serialize[]>([]);
  const active = shallowRef<JobSummary_Serialize[]>([]);
  const recent = shallowRef<JobSummary_Serialize[]>([]);
  const totalJobs = ref(0);
  const loaded = ref(false);
  const failed = ref(false);
  /** The service answered at least once: what is on screen is real, if maybe old. */
  const answered = ref(false);
  /** Finished-with-warnings transfers opened since they finished (per window, not engine state). */
  const seen = ref<Set<string>>(readSeen());

  function readSeen(): Set<string> {
    try {
      return new Set(JSON.parse(localStorage.getItem(SEEN_KEY) ?? "[]") as string[]);
    } catch {
      return new Set();
    }
  }
  function markSeen(id: string) {
    if (seen.value.has(id)) return;
    seen.value = new Set([...seen.value, id]);
    try {
      localStorage.setItem(SEEN_KEY, JSON.stringify([...seen.value].slice(-500)));
    } catch {
      // Seen for this session only.
    }
  }

  async function load() {
    try {
      const [needs, running, finished, all] = await Promise.all([
        api.listJobs(filter(["needs_you"]), 10, 0),
        api.listJobs(filter(["active"]), 20, 0),
        api.listJobs(filter(["finished", "cancelled"]), 8, 0),
        api.listJobs(filter([]), 1, 0),
      ]);
      for (const j of [...needs.items, ...running.items, ...finished.items]) jobs.byId[j.id] = j;
      attention.value = needs.items;
      active.value = running.items;
      recent.value = finished.items;
      totalJobs.value = all.total;
      failed.value = false;
      answered.value = true;
    } catch {
      failed.value = true;
    } finally {
      loaded.value = true;
    }
  }

  useConnection().onReconnect(() => void load());

  // Coalesce the reloads a burst of state changes would cause (at most once a second).
  let timer: ReturnType<typeof setTimeout> | undefined;
  function reloadSoon() {
    if (timer) return;
    timer = setTimeout(() => {
      timer = undefined;
      void load();
    }, 1000);
  }

  /** Needs-you, plus long retries among the running and unopened warnings among the finished. */
  const needs = computed(() => {
    const live = (j: JobSummary_Serialize) => jobs.byId[j.id] ?? j;
    const extra = [
      ...active.value.map(live).filter((j) => needsYou(j)),
      ...recent.value.map(live).filter((j) => j.state === "completed_with_warnings" && !seen.value.has(j.id)),
    ];
    const ids = new Set(attention.value.map((j) => j.id));
    return [...attention.value.map(live), ...extra.filter((j) => !ids.has(j.id))];
  });
  /** Running and paused ones: a paused transfer stays in sight (with "Resume") until it ends. One
   *  the engine paused with a reason is under Needs you instead. */
  const sending = computed(() => active.value.map((j) => jobs.byId[j.id] ?? j).filter((j) => !needsYou(j)));

  return { attention, active, recent, totalJobs, loaded, failed, answered, needs, sending, load, reloadSoon, markSeen };
});
