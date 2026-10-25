// The "New transfer" wizard (docs/06). The job is created once the destination is chosen
// (the service needs a profile to create one) and analysed right away; the analysis step then
// shows the scan's running total.
import { defineStore } from "pinia";
import { computed, ref, shallowRef, watch } from "vue";
import type {
  AnalysisSummary_Serialize,
  DecisionInput,
  JobSummary_Serialize,
  RuleCount,
  SendOptions,
  SourceInfo_Serialize,
} from "@/bindings";
import { api, errorMessage } from "@/lib/api";
import { askBatteryExemptionOnce } from "@/lib/platform";
import { useJobs } from "./jobs";
import { useProfiles } from "./profiles";

export type Step = 1 | 2 | 3 | 4 | 5;
/** The steps before sending; step 5 is the result, shown once sent (wording quotes this number). */
export const SEND_STEPS = 4;

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

  // ---- what each chosen source is (docs/design/specs/D02): checked as the list changes ----
  /** By path; a path missing here is still being checked. */
  const sourceInfo = shallowRef<Record<string, SourceInfo_Serialize>>({});
  const inspecting = ref(false);
  const inspectError = ref<string | null>(null);
  let inspectSeq = 0;
  let inspectTimer: ReturnType<typeof setTimeout> | undefined;

  async function inspect() {
    clearTimeout(inspectTimer);
    const seq = ++inspectSeq;
    const paths = [...sources.value];
    if (!paths.length) {
      sourceInfo.value = {};
      inspecting.value = false;
      inspectError.value = null;
      return;
    }
    inspecting.value = true;
    try {
      const found = await api.inspectSources(paths);
      if (seq !== inspectSeq) return; // the list changed while it ran
      // `covered_by` is an index into the list as sent; keep it as the path it names.
      const next: Record<string, SourceInfo_Serialize> = {};
      for (const info of found) next[info.path] = info;
      sourceInfo.value = next;
      coveredBy.value = Object.fromEntries(
        found.filter((i) => i.covered_by != null).map((i) => [i.path, paths[i.covered_by!]]),
      );
      inspectError.value = null;
    } catch (e) {
      if (seq !== inspectSeq) return;
      inspectError.value = errorMessage(e);
    } finally {
      if (seq === inspectSeq) inspecting.value = false;
    }
  }
  /** Path → the chosen source that already contains it. */
  const coveredBy = shallowRef<Record<string, string>>({});
  watch(
    () => sources.value.join("\n"),
    () => {
      clearTimeout(inspectTimer);
      inspectTimer = setTimeout(() => void inspect(), 150);
    },
  );

  /** Sources that stop the step: a problem, or already inside another chosen one. */
  const sourceBlockers = computed(() =>
    sources.value.filter((p) => !!sourceInfo.value[p]?.problem || (!!coveredBy.value[p] && sources.value.includes(coveredBy.value[p]))),
  );
  /** A failed check doesn't block: the analysis looks at everything again. */
  const canLeaveSource = computed(() => sources.value.length > 0 && sourceBlockers.value.length === 0);

  const pending = computed(() => ruleCounts.value.reduce((n, r) => n + r.pending, 0));
  const key = computed(() => JSON.stringify([sources.value, profileId.value]));
  const canAnalyse = computed(() => sources.value.length > 0 && profileId.value != null);
  const canReview = computed(() => !!jobId.value && !analysing.value && !analyseError.value && pending.value === 0);
  /** Choices not sent yet, which starting something else would drop. */
  const hasWork = computed(() => step.value < 5 && sources.value.length > 0);

  function addSources(paths: string[]) {
    const seen = new Set(sources.value);
    for (const p of paths) if (!seen.has(p)) sources.value.push(p);
  }

  /** Returns where it was, for undo. */
  function removeSource(path: string): number {
    const i = sources.value.indexOf(path);
    if (i >= 0) sources.value = sources.value.filter((p) => p !== path);
    return i;
  }

  /** Undo: puts removed sources back where they were (`at` ascending). */
  function restoreSources(removed: { path: string; at: number }[]) {
    const next = [...sources.value];
    for (const r of [...removed].sort((a, b) => a.at - b.at)) {
      if (!next.includes(r.path)) next.splice(Math.min(r.at, next.length), 0, r.path);
    }
    sources.value = next;
  }

  /** "Use the folder the link points to": in place, unless it is already chosen. */
  function replaceSource(old: string, next: string) {
    sources.value = sources.value.includes(next)
      ? sources.value.filter((p) => p !== old)
      : sources.value.map((p) => (p === old ? next : p));
  }

  function reset() {
    step.value = 1;
    sources.value = [];
    sourceInfo.value = {};
    coveredBy.value = {};
    inspecting.value = false;
    inspectError.value = null;
    inspectSeq++;
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
        // The old job was never sent: it is discarded (ADR 0030), so it neither looks queued nor
        // shows up as a cancellation the user never made. Nothing is deleted anywhere.
        if (jobId.value) await api.discardDraft(jobId.value).catch(() => undefined);
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
      void askBatteryExemptionOnce();
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
    hasWork,
    sourceInfo,
    coveredBy,
    inspecting,
    inspectError,
    sourceBlockers,
    canLeaveSource,
    inspect,
    addSources,
    removeSource,
    restoreSources,
    replaceSource,
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
