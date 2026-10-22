// The "New transfer" wizard (docs/06). The job is created once the destination is chosen
// (the service needs a profile to create one) and analysed right away; the analysis step then
// shows the scan's running total.
import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type {
  AnalysisSummary_Serialize,
  DecisionInput,
  JobSummary_Serialize,
  RuleCount,
  SendOptions,
} from "@/bindings";
import { api, errorMessage } from "@/lib/api";
import { useJobs } from "./jobs";
import { useProfiles } from "./profiles";

export type Step = 1 | 2 | 3 | 4 | 5;

function defaultOptions(): SendOptions {
  // Verifying by checksum is the safe default for a one-off migration.
  return { verify: true, concurrency: 8, target_dir: null };
}

export const useWizard = defineStore("wizard", () => {
  const step = ref<Step>(1);
  const sources = ref<string[]>([]);
  const profileId = ref<string | null>(null);
  const options = ref<SendOptions>(defaultOptions());

  const jobId = ref<string | null>(null);
  /** The sources and destination `jobId` was created from; a change means a new job and a new analysis. */
  const jobKey = ref<string | null>(null);
  /** The options the service has for `jobId`. Step 4 changes them in place (`setOptions`). */
  const jobOptions = ref<SendOptions | null>(null);
  const analysis = ref<AnalysisSummary_Serialize | null>(null);
  const analysing = ref(false);
  const analyseError = ref<string | null>(null);
  const ruleCounts = ref<RuleCount[]>([]);
  const sendError = ref<string | null>(null);
  const sending = ref(false);
  /** Bumped after every decision, so open issue lists can refresh their rows. */
  const decisionsVersion = ref(0);

  const pending = computed(() => ruleCounts.value.reduce((n, r) => n + r.pending, 0));
  const key = computed(() => JSON.stringify([sources.value, profileId.value]));
  const canAnalyse = computed(() => sources.value.length > 0 && profileId.value != null);
  const canReview = computed(() => !!jobId.value && !analysing.value && !analyseError.value && pending.value === 0);

  function addSources(paths: string[]) {
    const seen = new Set(sources.value);
    for (const p of paths) if (!seen.has(p)) sources.value.push(p);
  }

  function removeSource(path: string) {
    sources.value = sources.value.filter((p) => p !== path);
  }

  function reset() {
    step.value = 1;
    sources.value = [];
    profileId.value = null;
    options.value = defaultOptions();
    jobId.value = null;
    jobKey.value = null;
    jobOptions.value = null;
    analysis.value = null;
    analysing.value = false;
    analyseError.value = null;
    ruleCounts.value = [];
    sendError.value = null;
    sending.value = false;
  }

  /** Creates the job (if the sources or the destination changed) and analyses it. */
  async function analyse(force = false) {
    if (!canAnalyse.value) return;
    if (!force && jobKey.value === key.value && (analysis.value || analysing.value)) return;
    analysing.value = true;
    analyseError.value = null;
    analysis.value = null;
    ruleCounts.value = [];
    try {
      if (jobKey.value !== key.value || !jobId.value) {
        // The old job was never sent; cancelling it keeps it from looking queued in Jobs.
        // Nothing is deleted anywhere.
        if (jobId.value) await api.cancel(jobId.value).catch(() => undefined);
        jobId.value = await api.createJob([...sources.value], profileId.value!, { ...options.value });
        jobKey.value = key.value;
        jobOptions.value = { ...options.value };
      }
      await saveOptions();
      const id = jobId.value;
      void useJobs().refresh(id);
      const summary = await api.analyse(id);
      if (jobId.value !== id) return; // superseded while it ran
      analysis.value = summary;
      ruleCounts.value = summary.rules;
    } catch (e) {
      analyseError.value = errorMessage(e);
    } finally {
      analysing.value = false;
    }
  }

  async function refreshRuleCounts() {
    if (!jobId.value) return;
    ruleCounts.value = await api.ruleCounts(jobId.value, null);
  }

  async function decide(issueId: number, decision: DecisionInput) {
    if (!jobId.value) return;
    await api.decide(jobId.value, issueId, decision);
    decisionsVersion.value++;
    await refreshRuleCounts();
  }

  /** Applies one decision to every pending issue of `rule` (whose path contains `query`, when
   *  given) in one call; returns how many changed. */
  async function decideAll(rule: string, decision: DecisionInput, query: string | null = null): Promise<number> {
    if (!jobId.value) return 0;
    try {
      return await api.decideAll(jobId.value, rule, decision, query);
    } finally {
      decisionsVersion.value++;
      await refreshRuleCounts();
    }
  }

  /** One decision for each selected issue; the ones refused keep theirs (docs/design/specs/D08). */
  async function decideMany(ids: number[], decision: DecisionInput) {
    if (!jobId.value) return { changed: 0, refused: [] };
    try {
      return await api.decideMany(jobId.value, ids, decision);
    } finally {
      decisionsVersion.value++;
      await refreshRuleCounts();
    }
  }

  /** The rule whose issues are selected: one group at a time. */
  const selectionGroup = ref<string | null>(null);

  const optionsChanged = computed(
    () => !!jobOptions.value && JSON.stringify(jobOptions.value) !== JSON.stringify(options.value),
  );

  /** Gives the job the current options. The analysis and the decisions are kept. */
  async function saveOptions() {
    if (!jobId.value || !optionsChanged.value) return;
    const next = { ...options.value };
    await api.setOptions(jobId.value, next);
    jobOptions.value = next;
  }

  /** Step 4's checkbox: saves right away, so the job in Jobs shows the choice too. */
  async function changeOptions() {
    sendError.value = null;
    try {
      await saveOptions();
      if (jobId.value) void useJobs().refresh(jobId.value);
    } catch (e) {
      sendError.value = errorMessage(e);
    }
  }

  async function send(): Promise<boolean> {
    if (!jobId.value) return false;
    sending.value = true;
    sendError.value = null;
    try {
      await saveOptions();
      await api.start(jobId.value);
      await useJobs().refresh(jobId.value);
      step.value = 5;
      return true;
    } catch (e) {
      sendError.value = errorMessage(e);
      return false;
    } finally {
      sending.value = false;
    }
  }

  /** Continue an unsent job (Draft, AwaitingApproval or Ready) from the Jobs screen. */
  async function openJob(job: JobSummary_Serialize) {
    reset();
    sources.value = [...job.sources];
    profileId.value = job.profile_id;
    options.value = { ...job.options };
    jobId.value = job.id;
    jobKey.value = key.value;
    jobOptions.value = { ...job.options };
    step.value = 3;
    void useProfiles().ensure(job.profile_id);
    if (job.state === "draft") {
      await analyse(true);
    } else {
      await refreshRuleCounts();
    }
  }

  return {
    step,
    sources,
    profileId,
    options,
    jobId,
    jobOptions,
    analysis,
    analysing,
    analyseError,
    ruleCounts,
    pending,
    sendError,
    sending,
    decisionsVersion,
    canAnalyse,
    canReview,
    addSources,
    removeSource,
    reset,
    analyse,
    refreshRuleCounts,
    decide,
    decideAll,
    decideMany,
    selectionGroup,
    changeOptions,
    send,
    openJob,
  };
});
