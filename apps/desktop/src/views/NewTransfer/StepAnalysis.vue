<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import TourButton from "@/components/TourButton.vue";
import { useTour } from "@/stores/tour";
import BulkDecision from "@/components/BulkDecision.vue";
import IssueGroup from "@/components/IssueGroup.vue";
import NoticeBox from "@/components/NoticeBox.vue";
import PreflightList from "@/components/PreflightList.vue";
import ProgressBar from "@/components/ProgressBar.vue";
import TransferComposition from "@/components/TransferComposition.vue";
import { useProfiles } from "@/stores/profiles";
import DetailsPanel from "@/components/DetailsPanel.vue";
import HelpTip from "@/components/HelpTip.vue";
import { useFmt } from "@/composables/useFmt";
import { bulkAcceptable } from "@/lib/decisions";
import { request } from "@/lib/requests";
import { api } from "@/lib/api";
import { useRouter } from "vue-router";
import { useJobs } from "@/stores/jobs";
import { useWizard } from "@/stores/wizard";

const wizard = useWizard();
const jobs = useJobs();
const fmt = useFmt();
const profiles = useProfiles();
const destinationName = computed(() => (wizard.profileId ? profiles.byId[wizard.profileId]?.name : undefined));
/** The destination's space: the analysis just measured it, so the kept numbers are fresh. */
const storage = computed(() => (wizard.profileId ? profiles.storage[wizard.profileId] : undefined));
watch(
  () => [wizard.profileId, wizard.analysis] as const,
  ([id]) => {
    if (id) void profiles.loadStorage(id);
  },
  { immediate: true },
);

// Garbled names at the source, and transfers already made to this destination: those may have
// left names like these there. Repair destination fixes them in place (docs/04).
const router = useRouter();
const repairHint = ref(false);
watch(
  () => [wizard.profileId, wizard.ruleCounts.some((r) => r.rule === "mojibake" && r.total > 0)] as const,
  async ([id, garbled]) => {
    repairHint.value = false;
    if (!id || !garbled) return;
    try {
      const used = await api.listJobs({ query: null, groups: ["finished"], profile_id: id, reclaimable: false, sort: "newest" }, 1, 0);
      repairHint.value = used.total > 0;
    } catch {
      // No hint.
    }
  },
  { immediate: true },
);

// The guide starts by itself once, when there is an analysis to point at.
const tour = useTour();
watch(
  () => !!wizard.jobId && !wizard.analysing && !wizard.analyseError,
  async (ready) => {
    if (!ready) return;
    await nextTick();
    tour.maybeStart("analysis");
  },
  { immediate: true },
);
onBeforeUnmount(() => tour.stop("analysis"));

/** "Review one by one": every group opens; until then only those the button leaves out do. */
const reviewing = ref(false);
async function review() {
  reviewing.value = true;
  await nextTick();
  document.querySelector<HTMLElement>(".issue-group.pending .group-head")?.focus();
}

const scan = computed(() => (wizard.jobId ? jobs.live[wizard.jobId]?.scan : undefined));
const job = computed(() => (wizard.jobId ? jobs.byId[wizard.jobId] : undefined));
const a = computed(() => wizard.analysis);
/** Without a fresh analysis (a job reopened from Jobs), the persisted counts stand in. */
const totals = computed(() => a.value ?? (job.value ? { files: job.value.counts.files, dirs: job.value.counts.dirs, bytes: job.value.counts.bytes } : null));
const free = computed(() => a.value?.preflight.find((p) => p.source === "transport")?.free_bytes ?? null);
const tooBig = computed(() => free.value != null && a.value != null && a.value.bytes > free.value);

const details = computed(() => {
  if (!wizard.jobId) return [];
  const id = wizard.jobId;
  return [
    request("create_job", { sources: wizard.sources, profile_id: wizard.profileId, options: wizard.jobOptions }),
    request("analyse", { job_id: id }),
    ...wizard.ruleCounts.map((r) => request("issues", { job_id: id, rule: r.rule, after_id: null, limit: 101 })),
  ];
});
</script>

<template>
  <div class="step">
    <div class="title-row">
      <h2>{{ $t("analysis.title") }}</h2>
      <TourButton tour="analysis" />
    </div>
    <p class="lead-text">{{ $t("analysis.intro") }}</p>

    <div v-if="wizard.analysing" class="card counting" aria-live="polite">
      <strong>{{ $t("analysis.counting") }}</strong>
      <span class="small muted">{{ $t("analysis.countingHint") }}</span>
      <p class="total">
        {{ $t("source.total", { files: fmt.count(scan?.files ?? 0), size: fmt.bytes(scan?.bytes ?? 0) }) }}
      </p>
      <ProgressBar indeterminate />
      <p v-if="scan?.current" class="small muted ellipsis mono">{{ scan.current }}</p>
    </div>

    <div v-else-if="wizard.analyseError" class="card">
      <p class="error">{{ wizard.analyseError }}</p>
      <p class="small muted">{{ $t("analysis.errorHint") }}</p>
      <button class="btn" @click="wizard.analyse(true)">{{ $t("common.retry") }}</button>
    </div>

    <template v-else-if="wizard.jobId">
      <BulkDecision v-if="wizard.pending > 0" data-tour="analysis-decide" :rules="wizard.ruleCounts" @review="review" />
      <NoticeBox v-else-if="tooBig" tone="warn" icon="warning" role="status"><p>{{ $t("analysis.tooBigHint") }}</p></NoticeBox>
      <NoticeBox v-else tone="ok" icon="checkCircle" role="status">
        <p>{{ wizard.ruleCounts.length ? $t("analysis.guideFixed") : $t("analysis.guideOk") }}</p>
      </NoticeBox>

      <div class="card summary" data-tour="analysis-summary">
        <div class="stats">
          <div><span class="big">{{ fmt.count(totals?.files) }}</span><span class="small muted">{{ $t("analysis.files") }}</span></div>
          <div><span class="big">{{ fmt.count(totals?.dirs) }}</span><span class="small muted">{{ $t("analysis.dirs") }}</span></div>
          <div><span class="big">{{ fmt.bytes(totals?.bytes) }}</span><span class="small muted">{{ $t("analysis.size") }}</span></div>
          <div v-if="a"><span class="big">{{ fmt.count(a.skipped_symlinks) }}</span><span class="small muted">{{ $t("analysis.symlinks") }}<HelpTip :text="$t('analysis.symlinksHelp')" :label="$t('analysis.symlinks')" /></span></div>
        </div>
        <p v-if="tooBig" class="error small">{{ $t("analysis.tooBig", { free: fmt.bytes(free), size: fmt.bytes(a?.bytes) }) }}</p>
        <details v-if="a?.scan_errors.length" class="scan-errors">
          <summary class="warn-text small">{{ $t("analysis.scanErrors", { n: a.scan_errors.length }, a.scan_errors.length) }}</summary>
          <p class="small muted">{{ $t("analysis.scanErrorsHelp") }}</p>
          <ul class="small mono">
            <li v-for="(e, i) in a.scan_errors.slice(0, 200)" :key="i">{{ e }}</li>
          </ul>
        </details>
        <details v-if="a?.preflight.length" class="pre" :open="a.preflight.some((p) => p.checks.some((c) => c.status !== 'ok'))">
          <summary class="small">{{ $t("analysis.preflight") }} <span class="muted">— {{ $t("analysis.preflightHelp") }}</span></summary>
          <PreflightList :views="a.preflight" />
        </details>
        <p v-if="!a" class="small muted">
          {{ $t("analysis.reopened") }}
          <button class="btn btn-small" @click="wizard.analyse(true)">{{ $t("analysis.rerun") }}</button>
        </p>
      </div>

      <TransferComposition
        :job-id="wizard.jobId"
        :free="storage?.free_bytes ?? free"
        :total="storage?.total_bytes ?? null"
        :used="storage?.used_bytes ?? null"
        :destination="destinationName"
      />

      <h3 data-tour="analysis-names">{{ $t("analysis.names") }}<HelpTip :text="$t('analysis.namesHelp')" :example="$t('analysis.namesExample')" :label="$t('analysis.names')" /></h3>
      <details v-if="wizard.ruleCounts.length" class="legend card" :open="wizard.pending > 0">
        <summary class="small"><strong>{{ $t("analysis.legendTitle") }}</strong></summary>
        <dl class="small">
          <dt>{{ $t("issues.accept") }}</dt><dd>{{ $t("issues.acceptTip") }} <span class="badge badge-ok">{{ $t("common.recommended") }}</span></dd>
          <dt>{{ $t("issues.keep") }}</dt><dd>{{ $t("issues.keepTip") }}</dd>
          <dt>{{ $t("issues.skip") }}</dt><dd>{{ $t("issues.skipTip") }}</dd>
          <dt>{{ $t("issues.edit") }}</dt><dd>{{ $t("issues.editTip") }}</dd>
        </dl>
      </details>
      <p v-if="!wizard.ruleCounts.length" class="card ok-text"><AppIcon name="check" /> {{ $t("analysis.noIssues") }}</p>
      <NoticeBox v-if="repairHint" icon="renamed" class="repair-hint">
        <p>{{ $t("analysis.repairHint", { destination: destinationName ?? "" }) }}</p>
        <div><button class="btn btn-small" @click="router.push({ name: 'destinations' })">{{ $t("repair.button") }}</button></div>
      </NoticeBox>
      <div class="groups">
        <IssueGroup
          v-for="r in wizard.ruleCounts"
          :key="`${wizard.jobId}-${r.rule}-${reviewing}`"
          :job-id="wizard.jobId"
          :rule="r"
          :start-open="r.pending > 0 && (reviewing || !bulkAcceptable(r))"
        />
      </div>
    </template>

    <DetailsPanel :lines="details" />

    <div class="footer sticky" data-tour="analysis-next">
      <button class="btn" :disabled="wizard.analysing" @click="wizard.step = 2"><AppIcon name="back" /> {{ $t("common.back") }}</button>
      <span class="grow" />
      <span v-if="wizard.analysing" class="why-disabled">{{ $t("analysis.waitHint") }}</span>
      <span v-else-if="wizard.pending > 0" class="small warn-text">{{ $t("analysis.pendingHint", { n: fmt.count(wizard.pending) }, wizard.pending) }}</span>
      <button class="btn btn-primary" :disabled="!wizard.canReview" @click="wizard.step = 4">{{ $t("common.next") }} <AppIcon name="next" /></button>
    </div>
  </div>
</template>

<style scoped>
.title-row { display: flex; gap: var(--sp-2); align-items: center; }
.title-row h2 { margin: 0; }
.counting { display: grid; gap: 0.5rem; }
.total { font-size: var(--fs-xl); font-weight: 600; margin: 0; }
.stats { display: flex; gap: 2rem; flex-wrap: wrap; }
.stats > div { display: grid; }
.big { font-size: var(--fs-xl); font-weight: 600; }
.scan-errors, .pre { margin-top: 0.75rem; }
.scan-errors ul { max-height: 12rem; overflow: auto; }
h3 { margin: 1.5rem 0 0.5rem; display: flex; align-items: center; }
.stats .small { display: inline-flex; align-items: center; }
.legend { margin-bottom: 0.75rem; padding: 0.6rem 1rem; }
.legend summary { cursor: pointer; }
.legend dl { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 0.35rem 1rem; margin: 0.6rem 0 0; }
.legend dt { font-weight: 600; }
.legend dd { margin: 0; color: var(--text-2); }
.groups { display: grid; gap: 0.75rem; }
</style>
