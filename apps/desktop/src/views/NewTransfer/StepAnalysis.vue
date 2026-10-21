<script setup lang="ts">
import { computed } from "vue";
import IssueGroup from "@/components/IssueGroup.vue";
import PreflightList from "@/components/PreflightList.vue";
import ProgressBar from "@/components/ProgressBar.vue";
import DetailsPanel from "@/components/DetailsPanel.vue";
import { useFmt } from "@/composables/useFmt";
import { request } from "@/lib/requests";
import { useJobs } from "@/stores/jobs";
import { useWizard } from "@/stores/wizard";

const wizard = useWizard();
const jobs = useJobs();
const fmt = useFmt();

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
    <h2>{{ $t("analysis.title") }}</h2>

    <div v-if="wizard.analysing" class="card counting" aria-live="polite">
      <strong>{{ $t("analysis.counting") }}</strong>
      <p class="total">
        {{ $t("source.total", { files: fmt.count(scan?.files ?? 0), size: fmt.bytes(scan?.bytes ?? 0) }) }}
      </p>
      <ProgressBar indeterminate />
      <p v-if="scan?.current" class="small muted ellipsis mono">{{ scan.current }}</p>
    </div>

    <div v-else-if="wizard.analyseError" class="card">
      <p class="error">{{ wizard.analyseError }}</p>
      <button class="btn" @click="wizard.analyse(true)">{{ $t("common.retry") }}</button>
    </div>

    <template v-else-if="wizard.jobId">
      <div class="card summary">
        <div class="stats">
          <div><span class="big">{{ fmt.count(totals?.files) }}</span><span class="small muted">{{ $t("analysis.files") }}</span></div>
          <div><span class="big">{{ fmt.count(totals?.dirs) }}</span><span class="small muted">{{ $t("analysis.dirs") }}</span></div>
          <div><span class="big">{{ fmt.bytes(totals?.bytes) }}</span><span class="small muted">{{ $t("analysis.size") }}</span></div>
          <div v-if="a"><span class="big">{{ fmt.count(a.skipped_symlinks) }}</span><span class="small muted">{{ $t("analysis.symlinks") }}</span></div>
        </div>
        <p v-if="tooBig" class="error small">{{ $t("analysis.tooBig", { free: fmt.bytes(free), size: fmt.bytes(a?.bytes) }) }}</p>
        <details v-if="a?.scan_errors.length" class="scan-errors">
          <summary class="warn-text small">{{ $t("analysis.scanErrors", { n: a.scan_errors.length }, a.scan_errors.length) }}</summary>
          <ul class="small mono">
            <li v-for="(e, i) in a.scan_errors.slice(0, 200)" :key="i">{{ e }}</li>
          </ul>
        </details>
        <details v-if="a?.preflight.length" class="pre" :open="a.preflight.some((p) => p.checks.some((c) => c.status !== 'ok'))">
          <summary class="small">{{ $t("analysis.preflight") }}</summary>
          <PreflightList :views="a.preflight" />
        </details>
        <p v-if="!a" class="small muted">
          {{ $t("analysis.reopened") }}
          <button class="btn btn-small" @click="wizard.analyse(true)">{{ $t("analysis.rerun") }}</button>
        </p>
      </div>

      <h3>{{ $t("analysis.names") }}</h3>
      <p v-if="!wizard.ruleCounts.length" class="card ok-text">✓ {{ $t("analysis.noIssues") }}</p>
      <div class="groups">
        <IssueGroup v-for="r in wizard.ruleCounts" :key="`${wizard.jobId}-${r.rule}`" :job-id="wizard.jobId" :rule="r" />
      </div>
    </template>

    <DetailsPanel :lines="details" />

    <div class="footer">
      <button class="btn" :disabled="wizard.analysing" @click="wizard.step = 2">← {{ $t("common.back") }}</button>
      <span class="grow" />
      <span v-if="wizard.pending > 0" class="small warn-text">{{ $t("analysis.pendingHint", { n: fmt.count(wizard.pending) }, wizard.pending) }}</span>
      <button class="btn btn-primary" :disabled="!wizard.canReview" @click="wizard.step = 4">{{ $t("common.next") }} →</button>
    </div>
  </div>
</template>

<style scoped>
.counting { display: grid; gap: 0.5rem; }
.total { font-size: 1.4rem; font-weight: 600; margin: 0; }
.stats { display: flex; gap: 2rem; flex-wrap: wrap; }
.stats > div { display: grid; }
.big { font-size: 1.4rem; font-weight: 600; }
.scan-errors, .pre { margin-top: 0.75rem; }
.scan-errors ul { max-height: 12rem; overflow: auto; }
h3 { margin: 1.5rem 0 0.5rem; }
.groups { display: grid; gap: 0.75rem; }
</style>
