<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import DetailsPanel from "@/components/DetailsPanel.vue";
import JobProgress from "@/components/JobProgress.vue";
import { useFmt } from "@/composables/useFmt";
import { errorMessage } from "@/lib/api";
import { request } from "@/lib/requests";
import { RUNNING, TERMINAL, useJobs } from "@/stores/jobs";
import { profileWhere, useProfiles } from "@/stores/profiles";
import { useWizard } from "@/stores/wizard";

const props = defineProps<{ id: string }>();
const jobs = useJobs();
const profiles = useProfiles();
const wizard = useWizard();
const router = useRouter();
const fmt = useFmt();
const { t } = useI18n();

const job = computed(() => jobs.byId[props.id]);
const live = computed(() => jobs.live[props.id]);
const profile = computed(() => (job.value ? profiles.byId[job.value.profile_id] : undefined));
const busy = ref(false);
const error = ref<string | null>(null);
const confirmCancel = ref(false);
const notFound = ref(false);

async function load() {
  const j = await jobs.refresh(props.id);
  notFound.value = !j;
  if (j) await profiles.ensure(j.profile_id);
}
onMounted(load);
watch(() => props.id, load);

const state = computed(() => job.value?.state);
const canPause = computed(() => !!state.value && RUNNING.includes(state.value));
const canResume = computed(() => state.value === "paused");
const canCancel = computed(() => !!state.value && !TERMINAL.includes(state.value));
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
  await wizard.openJob(job.value);
  await router.push({ name: "new" });
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
        <button class="link small" @click="router.push({ name: 'jobs' })">← {{ $t("jobs.title") }}</button>
        <h1>{{ $t("jobDetail.title") }}</h1>
      </div>
      <div class="actions">
        <button v-if="unsent" class="btn btn-primary" @click="continueInWizard">{{ $t("jobDetail.continue") }}</button>
        <button v-if="canPause" class="btn" :disabled="busy" @click="act(() => jobs.pause(id))">⏸ {{ $t("jobDetail.pause") }}</button>
        <button v-if="canResume" class="btn btn-primary" :disabled="busy" @click="act(() => jobs.resume(id))">▶ {{ $t("jobDetail.resume") }}</button>
        <button v-if="canCancel" class="btn btn-danger" :disabled="busy" @click="confirmCancel = true">{{ $t("jobDetail.cancel") }}</button>
      </div>
    </header>

    <p v-if="notFound" class="card error">{{ $t("jobDetail.notFound") }}</p>
    <template v-else-if="job">
      <p v-if="error" class="error">{{ error }}</p>
      <div class="card"><JobProgress :job-id="id" /></div>

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
          <dl class="kv">
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

      <section class="card">
        <h2>{{ $t("jobDetail.log") }}</h2>
        <p class="small muted">{{ $t("jobDetail.logNote") }}</p>
        <ol v-if="live?.log.length" class="log small">
          <li v-for="(l, i) in [...live.log].reverse()" :key="i" :class="`log-${l.kind}`">
            <time class="muted mono">{{ new Date(l.at).toLocaleTimeString() }}</time> {{ l.text }}
          </li>
        </ol>
        <p v-else class="small muted">{{ $t("jobDetail.logEmpty") }}</p>
      </section>

      <DetailsPanel :lines="details" />
    </template>
    <p v-else class="muted">{{ $t("common.loading") }}</p>

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
.actions { display: flex; gap: 0.5rem; flex-wrap: wrap; }
.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 1rem; margin: 1rem 0; }
h2 { font-size: 1rem; margin-top: 0; }
.kv { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 0.35rem 1rem; margin: 0; }
dt { color: var(--text-2); }
dd { margin: 0; min-width: 0; }
.block { display: block; }
.log { list-style: none; padding: 0; margin: 0; max-height: 18rem; overflow: auto; }
.log li { padding: 0.2rem 0; border-bottom: 1px solid var(--border); }
.log-failure, .log-problem { color: var(--danger); }
.log-retry { color: var(--warn-strong); }
.link { all: unset; cursor: pointer; color: var(--accent); }
</style>
