<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import AccessDialog from "@/components/AccessDialog.vue";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import DetailsPanel from "@/components/DetailsPanel.vue";
import FilesTab from "@/components/FilesTab.vue";
import JobActions from "@/components/JobActions.vue";
import ReclaimPanel from "@/components/ReclaimPanel.vue";
import ReportDialog from "@/components/ReportDialog.vue";
import JobLog from "@/components/JobLog.vue";
import { continueJob, defaultLabel, jobLabel } from "@/lib/jobs";
import JobProgress from "@/components/JobProgress.vue";
import TransferQueue from "@/components/TransferQueue.vue";
import { useFmt } from "@/composables/useFmt";
import { errorMessage } from "@/lib/api";
import { request } from "@/lib/requests";
import { RUNNING, TERMINAL, useJobs } from "@/stores/jobs";
import { profileWhere, useProfiles } from "@/stores/profiles";
import { useHome } from "@/stores/home";

const props = defineProps<{ id: string }>();
const jobs = useJobs();
const profiles = useProfiles();
const router = useRouter();
const route = useRoute();
const fmt = useFmt();
const { t } = useI18n();

const job = computed(() => jobs.byId[props.id]);
const profile = computed(() => (job.value ? profiles.byId[job.value.profile_id] : undefined));
/** docs/07: "Remove access" is offered once a migration to a destination with filebridge's own
 *  key has finished. */
const offerRemoveAccess = computed(() => {
  const b = profile.value?.destination.backend;
  return !!job.value && ["completed", "completed_with_warnings"].includes(job.value.state) && b?.kind === "sftp" && !!b.key;
});
const removingAccess = ref(false);
const busy = ref(false);
const error = ref<string | null>(null);
const confirmCancel = ref(false);
const notFound = ref(false);
const reportOpen = ref(false);

/** Tabs of Transfer detail (docs/design/02, "Transfer detail: tabs"); kept in `?tab=`. */
const TABS = ["overview", "files", "log"] as const;
type Tab = (typeof TABS)[number];
const tab = computed<Tab>(() => (TABS as readonly string[]).includes(route.query.tab as string) ? (route.query.tab as Tab) : "overview");
function selectTab(next: Tab) {
  // Replace, not push: Back from a tab goes back to Transfers.
  // The Files tab's filters and drawer belong to it alone.
  void router.replace({ query: { tab: next === "overview" ? undefined : next } });
}
function onTabKey(e: KeyboardEvent) {
  const i = TABS.indexOf(tab.value);
  const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
  if (!step) return;
  e.preventDefault();
  const next = TABS[(i + step + TABS.length) % TABS.length];
  selectTab(next);
  document.getElementById(`tab-${next}`)?.focus();
}

async function load() {
  const j = await jobs.refresh(props.id);
  notFound.value = !j;
  // Opened: Home stops listing its warnings under "Needs you".
  if (j) useHome().markSeen(j.id);
  if (j) await profiles.ensure(j.profile_id);
}
onMounted(load);
watch(() => props.id, load);

const state = computed(() => job.value?.state);
const canPause = computed(() => !!state.value && RUNNING.includes(state.value));
const canResume = computed(() => state.value === "paused");
const canCancel = computed(() => !!state.value && !TERMINAL.includes(state.value));
/** Files that need a look, for the Files tab's badge. */
const problems = computed(() => (job.value ? job.value.counts.failed + job.value.counts.mismatch : 0));
const unsent = computed(() => state.value === "draft" || state.value === "awaiting_approval" || state.value === "ready");

async function act(fn: () => Promise<void>) {
  busy.value = true;
  error.value = null;
  try {
    await fn();
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}

async function doCancel() {
  await act(() => jobs.cancel(props.id));
  confirmCancel.value = false;
}

async function continueInWizard() {
  if (!job.value) return;
  await continueJob(job.value, router);
}

const cancelConsequences = computed(() => {
  const c = job.value?.counts;
  return [
    t("jobDetail.cancelStops"),
    t("jobDetail.cancelKeeps", { n: fmt.count(c?.done ?? 0) }),
    t("jobDetail.cancelNoDelete"),
    t("jobDetail.cancelFinal"),
  ];
});

const details = computed(() => [
  request("job", { job_id: props.id }),
  ...(canPause.value ? [request("pause", { job_id: props.id })] : []),
  ...(canResume.value ? [request("resume", { job_id: props.id })] : []),
  ...(canCancel.value ? [request("cancel", { job_id: props.id })] : []),
]);
</script>

<template>
  <div>
    <header class="page-head">
      <div>
        <button class="link small" @click="router.push({ name: 'jobs' })"><AppIcon name="back" /> {{ $t("jobs.title") }}</button>
        <h1>{{ job ? jobLabel(job) : $t("jobDetail.title") }}</h1>
        <p v-if="job?.label" class="small muted sub">{{ defaultLabel(job.sources) }}</p>
      </div>
      <div class="actions">
        <button v-if="unsent" class="btn btn-primary" :title="$t('jobDetail.continueTip')" @click="continueInWizard">{{ $t(job.kind === "repair" ? "repair.continue" : "jobDetail.continue") }}</button>
        <button v-if="canPause" class="btn" :title="$t('jobDetail.pauseTip')" :disabled="busy" @click="act(() => jobs.pause(id))"><AppIcon name="pause" /> {{ $t("jobDetail.pause") }}</button>
        <button v-if="canResume" class="btn btn-primary" :title="$t('jobDetail.resumeTip')" :disabled="busy" @click="act(() => jobs.resume(id))"><AppIcon name="play" /> {{ $t("jobDetail.resume") }}</button>
        <button v-if="canCancel" class="btn btn-danger" :title="$t('jobDetail.cancelTip')" :disabled="busy" @click="confirmCancel = true">{{ $t("jobDetail.cancel") }}</button>
        <JobActions v-if="job" :job="job" @export-report="reportOpen = true" />
      </div>
    </header>

    <p v-if="notFound" class="card error">{{ $t("jobDetail.notFound") }}</p>
    <template v-else-if="job">
      <p v-if="error" class="error">{{ error }}</p>
      <p v-if="unsent" class="guide"><AppIcon class="guide-icon" name="pointing" :size="20" /><span>{{ $t("jobDetail.unsentHint") }}</span></p>
      <div class="tabs" role="tablist" :aria-label="$t('jobDetail.tabs')" @keydown="onTabKey">
        <button
          v-for="t in TABS"
          :id="`tab-${t}`"
          :key="t"
          class="tab"
          role="tab"
          :aria-selected="tab === t"
          :aria-controls="`panel-${t}`"
          :tabindex="tab === t ? 0 : -1"
          @click="selectTab(t)"
        >
          {{ $t(`jobDetail.tab.${t}`) }}
          <span v-if="t === 'files' && problems" class="badge badge-danger num" :aria-label="$t('files.problemsBadge', { n: problems }, problems)">{{ fmt.count(problems) }}</span>
        </button>
      </div>

      <div v-if="tab === 'overview'" id="panel-overview" role="tabpanel" aria-labelledby="tab-overview">
        <div class="card"><JobProgress :job-id="id" /></div>
        <div v-if="['transferring', 'retrying', 'paused'].includes(job.state)" class="card queue-card"><TransferQueue :job-id="id" /></div>

        <div class="grid">
          <section class="card">
            <h2>{{ $t("jobDetail.what") }}</h2>
            <dl class="kv">
              <dt>{{ $t("review.sources") }}</dt>
              <dd><span v-for="s in job.sources" :key="s" class="mono small block ellipsis" :title="s">{{ s }}</span></dd>
              <dt>{{ $t("review.destination") }}</dt>
              <dd>{{ profile?.name ?? job.profile_id }} <span v-if="profile" class="mono small block">{{ profileWhere(profile) }}</span></dd>
              <dt>{{ $t("jobDetail.created") }}</dt>
              <dd>{{ fmt.date(job.created_at) }}</dd>
              <dt>{{ $t("jobDetail.finished") }}</dt>
              <dd>{{ fmt.date(job.finished_at) }}</dd>
              <dt>{{ $t("review.verify") }}</dt>
              <dd>{{ job.options.verify ? $t("common.yes") : $t("common.no") }}</dd>
              <dt>{{ $t("jobs.col.retries") }}</dt>
              <dd>{{ job.attempt }}<template v-if="job.next_retry_at"> · {{ $t("jobDetail.nextRetry", { at: fmt.date(job.next_retry_at) }) }}</template></dd>
            </dl>
          </section>
          <section class="card">
            <h2>{{ $t("jobDetail.counts") }}</h2>
            <dl class="kv num">
              <dt>{{ $t("analysis.files") }}</dt><dd>{{ fmt.count(job.counts.files) }}</dd>
              <dt>{{ $t("analysis.dirs") }}</dt><dd>{{ fmt.count(job.counts.dirs) }}</dd>
              <dt>{{ $t("analysis.size") }}</dt><dd>{{ fmt.bytes(job.counts.bytes) }}</dd>
              <dt>{{ $t("counts.pending") }}</dt><dd>{{ fmt.count(job.counts.pending) }}</dd>
              <dt>{{ $t("counts.done") }}</dt><dd>{{ fmt.count(job.counts.done) }}</dd>
              <dt>{{ $t("counts.failed") }}</dt><dd>{{ fmt.count(job.counts.failed) }}</dd>
              <dt>{{ $t("counts.skipped") }}</dt><dd>{{ fmt.count(job.counts.skipped) }}</dd>
              <dt>{{ $t("counts.verified") }}</dt><dd>{{ fmt.count(job.counts.verified_ok) }}</dd>
              <dt>{{ $t("counts.mismatch") }}</dt><dd>{{ fmt.count(job.counts.mismatch) }}</dd>
            </dl>
          </section>
        </div>

        <ReclaimPanel v-if="TERMINAL.includes(job.state)" :job-id="id" />
        <section v-if="offerRemoveAccess" class="card access-offer">
          <AppIcon name="lock" :size="22" class="muted" />
          <p class="small">{{ $t("access.offer", { name: profile?.name ?? "" }) }}</p>
          <button class="btn btn-small" @click="removingAccess = true">{{ $t("access.remove.button") }}</button>
        </section>
        <AccessDialog :profile="removingAccess ? profile ?? null : null" action="remove" @close="removingAccess = false" />
      </div>

      <div v-else-if="tab === 'files'" id="panel-files" role="tabpanel" aria-labelledby="tab-files">
        <p v-if="job.state === 'draft' || job.state === 'analysing'" class="card small muted">{{ $t("files.notAnalysed") }}</p>
        <FilesTab v-else :job-id="id" @export-report="reportOpen = true" />
      </div>
      <div v-else id="panel-log" role="tabpanel" aria-labelledby="tab-log">
        <JobLog :job-id="id" />
      </div>

      <DetailsPanel :lines="details" />
    </template>
    <p v-else class="muted">{{ $t("common.loading") }}</p>

    <ReportDialog :job-id="reportOpen ? id : null" @close="reportOpen = false" />
    <ConfirmDialog
      :open="confirmCancel"
      :title="$t('jobDetail.cancelTitle')"
      :consequences="cancelConsequences"
      :confirm-label="$t('jobDetail.cancelConfirm')"
      :cancel-label="$t('jobDetail.keepSending')"
      danger
      :busy="busy"
      @confirm="doCancel"
      @cancel="confirmCancel = false"
    />
  </div>
</template>

<style scoped>
.access-offer { display: flex; gap: 0.75rem; align-items: center; }
.access-offer p { margin: 0; flex: 1; }
.queue-card { margin-top: 1rem; }
.sub { margin: 0.1rem 0 0; }
.actions { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(300px, 100%), 1fr)); gap: 1rem; margin: 1rem 0; }
h2 { font-size: 1rem; margin-top: 0; }
.kv { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 0.35rem 1rem; margin: 0; }
dt { color: var(--text-2); }
dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
.block { display: block; }
.link { all: unset; cursor: pointer; color: var(--accent); display: inline-flex; gap: 0.3rem; align-items: center; }
.link:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.tabs { display: flex; gap: 0.25rem; border-bottom: 1px solid var(--border); margin: 0 0 1rem; }
.tab { display: inline-flex; gap: 0.4rem; align-items: center; font: inherit; background: none; border: 0; border-bottom: 2px solid transparent; padding: 0.5rem 0.9rem; margin-bottom: -1px; color: var(--text-2); cursor: pointer; }
.tab:hover { color: var(--text); }
.tab[aria-selected="true"] { color: var(--accent); border-bottom-color: var(--accent); font-weight: 600; }
.tab:focus-visible { outline: 2px solid var(--focus); outline-offset: -2px; }
/* The label and value side by side need room; in a narrow window the value goes under its label. */
@media (max-width: 399px) {
  .kv { grid-template-columns: minmax(0, 1fr); gap: 0.1rem; }
  .kv dd + dt { margin-top: 0.45rem; }
  .kv.num { grid-template-columns: max-content minmax(0, 1fr); gap: 0.35rem 1rem; }
  .kv.num dd + dt { margin-top: 0; }
}
</style>
