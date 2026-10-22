// Sources that can be freed safely (docs/design/specs/D10, ADR 0011: shown, never freed). Holds the
// reclaimable jobs for Home and the sidebar, and each job's status once asked.
import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import type { ReclaimStatus_Serialize as ReclaimStatus } from "@/bindings";
import { api } from "@/lib/api";

export const useReclaim = defineStore("reclaim", () => {
  const byJob = ref<Record<string, ReclaimStatus>>({});
  /** Jobs whose sources can be freed and aren't hidden. */
  const eligible = shallowRef<ReclaimStatus[]>([]);
  const checking = ref<Record<string, boolean>>({});

  async function loadEligible() {
    try {
      const page = await api.listJobs({ query: null, groups: [], profile_id: null, reclaimable: true, sort: "newest" }, 50, 0);
      const statuses = await Promise.all(page.items.map((j) => api.reclaimStatus(j.id)));
      for (const s of statuses) byJob.value[s.job_id] = s;
      eligible.value = statuses;
    } catch {
      // Home and the sidebar just show nothing reclaimable.
    }
  }

  async function status(jobId: string): Promise<ReclaimStatus | undefined> {
    try {
      byJob.value[jobId] = await api.reclaimStatus(jobId);
    } catch {
      // Keep what was shown.
    }
    return byJob.value[jobId];
  }

  async function recheck(jobId: string) {
    checking.value[jobId] = true;
    try {
      byJob.value[jobId] = await api.recheckSources(jobId);
    } finally {
      checking.value[jobId] = false;
      void loadEligible();
    }
  }

  async function dismiss(jobId: string, dismissed: boolean) {
    await api.dismissReclaim(jobId, dismissed);
    await status(jobId);
    await loadEligible();
  }

  const totalBytes = computed(() => eligible.value.reduce((n, s) => n + s.bytes, 0));

  return { byJob, eligible, checking, totalBytes, loadEligible, status, recheck, dismiss };
});
