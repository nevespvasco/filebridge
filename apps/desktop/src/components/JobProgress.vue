<script setup lang="ts">
// Live progress of one job: bytes and files, speed, ETA, retries, the send window, verify and
// integration stages.
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useFmt } from "@/composables/useFmt";
import { minutesToTime } from "@/lib/format";
import { RUNNING, TERMINAL, useJobs } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";
import { useSettings } from "@/stores/settings";
import ProgressBar from "./ProgressBar.vue";
import JobStateBadge from "./JobStateBadge.vue";

const props = defineProps<{ jobId: string }>();
const jobs = useJobs();
const settings = useSettings();
const profiles = useProfiles();
const fmt = useFmt();
const { t, te } = useI18n();

const job = computed(() => jobs.byId[props.jobId]);
const live = computed(() => jobs.live[props.jobId]);
const finished = computed(() => !!job.value && TERMINAL.includes(job.value.state));
/** Live numbers while sending; a finished job shows its final, persisted counts. */
const tr = computed(() => (finished.value ? undefined : live.value?.transfer));

/** Prefer the live event; fall back to the persisted counts. */
const bytesDone = computed(() => tr.value?.bytes_done ?? job.value?.counts.bytes_done ?? 0);
const bytesTotal = computed(() => tr.value?.bytes_total ?? job.value?.counts.bytes ?? 0);
const filesDone = computed(() => tr.value?.files_done ?? job.value?.counts.done ?? 0);
const filesTotal = computed(() => tr.value?.files_total ?? job.value?.counts.files ?? 0);
const running = computed(() => !!job.value && RUNNING.includes(job.value.state) && job.value.state !== "waiting");
const tone = computed(() => {
  const s = job.value?.state;
  if (s === "failed") return "danger";
  if (s === "completed") return "ok";
  if (s === "completed_with_warnings" || s === "paused" || s === "retrying" || s === "waiting") return "warn";
  return "accent";
});

/** When a waiting job goes again: from its event, else from the window in the settings. */
const windowText = computed(() => {
  const opens = live.value?.window?.opens_at;
  if (opens) return t("progress.window", { time: fmt.time(opens) });
  const w = settings.app.send_window;
  return w ? t("progress.windowSoon", { start: minutesToTime(w.start_minute), end: minutesToTime(w.end_minute) }) : "";
});

const destination = computed(() => (job.value ? profiles.byId[job.value.profile_id]?.name : undefined) ?? "—");
/** Measured during the send: what is left doesn't fit in the free space. Only a warning. */
const runningOut = computed(() => {
  const sp = live.value?.space;
  if (!sp || !running.value || sp.free_bytes >= sp.left_bytes) return "";
  return t("storage.runningOut", { name: destination.value, free: fmt.bytes(sp.free_bytes), left: fmt.bytes(sp.left_bytes) });
});
const full = computed(() => job.value?.state === "paused" && job.value.error_code === "destination_full");
/** Paused because a source can't be read (a disk unplugged, docs/design/specs/D02). */
const sourceGone = computed(() => job.value?.state === "paused" && job.value.error_code === "source_unavailable");

const integrationLabel = computed(() => {
  const i = live.value?.integration;
  if (!i) return "";
  const key = `integration.${i.step}`;
  return te(key) ? t(key) : i.step;
});
const integrationText = computed(() => {
  const i = live.value?.integration;
  if (!i) return "";
  let text = integrationLabel.value;
  if (i.total) text += `: ${fmt.count(i.done)} / ${fmt.count(i.total)}`;
  if (i.items) text += ` · ${t("progress.items", { n: fmt.count(i.items) })}`;
  return text;
});
const waitingLabel = computed(() => {
  const w = live.value?.integration?.waiting;
  if (!w) return "";
  const key = `integration.${w}`;
  return te(key) ? t(key) : w;
});
</script>

<template>
  <div v-if="job" class="job-progress">
    <div class="line">
      <JobStateBadge :state="job.state" />
      <span class="small muted num">
        {{ $t("progress.bytes", { done: fmt.bytes(bytesDone), total: fmt.bytes(bytesTotal) }) }} ·
        {{ $t("progress.files", { done: fmt.count(filesDone), total: fmt.count(filesTotal) }) }}
      </span>
      <span class="grow" />
      <span v-if="running && tr" class="small num">
        {{ fmt.rate(tr.bytes_per_sec) }} · {{ $t("progress.eta", { eta: fmt.duration(tr.eta_secs) }) }}
      </span>
    </div>
    <ProgressBar :value="fmt.percent(bytesDone, bytesTotal)" :tone="tone" />

    <p v-if="job.state === 'waiting' && windowText" class="small warn-text">{{ windowText }}</p>
    <p v-else-if="live?.retry && job.state === 'retrying'" class="small warn-text">
      {{ $t("progress.retry", { attempt: live.retry.attempt, secs: live.retry.in_secs }) }} — {{ live.retry.error }}
    </p>
    <p v-else-if="job.attempt > 0 && !finished" class="small muted">{{ $t("progress.retries", { n: job.attempt }) }}</p>
    <p v-if="runningOut" class="small warn-text" role="status">{{ runningOut }}</p>

    <div v-if="job.state === 'verifying' && live?.verify" class="stage">
      <span class="small">{{ $t("progress.verifying", { done: fmt.count(live.verify.checked), total: fmt.count(live.verify.total) }) }}</span>
      <span v-if="live.verify.mismatches" class="small warn-text"> · {{ $t("progress.mismatches", { n: live.verify.mismatches }) }}</span>
      <ProgressBar :value="fmt.percent(live.verify.checked, live.verify.total)" />
    </div>

    <div v-if="live?.integration && (job.state === 'integrating' || finished)" class="stage">
      <span class="small">{{ integrationText }}</span>
      <span v-if="waitingLabel" class="small warn-text"> · {{ waitingLabel }}</span>
      <div v-if="live.integration.current" class="small muted ellipsis">{{ live.integration.current }}</div>
      <ProgressBar
        v-if="job.state === 'integrating'"
        :value="fmt.percent(live.integration.done, live.integration.total)"
        :indeterminate="!live.integration.total"
      />
    </div>

    <div v-if="full" class="full" role="alert">
      <p class="small warn-text">{{ $t("storage.full", { name: destination }) }}</p>
      <p class="small">{{ $t("storage.fullHelp") }}</p>
      <p v-if="job.error" class="small muted ellipsis" :title="job.error">{{ job.error }}</p>
    </div>
    <div v-else-if="sourceGone" class="full" role="alert">
      <p class="small warn-text">{{ $t("sourceTools.gone") }}</p>
      <p class="small">{{ $t("sourceTools.goneHelp") }}</p>
      <p v-if="job.error" class="small muted ellipsis" :title="job.error">{{ job.error }}</p>
    </div>
    <p v-else-if="job.error" class="error small">{{ job.error }}</p>
  </div>
</template>

<style scoped>
.job-progress { display: grid; gap: 0.5rem; }
.line { display: flex; gap: 0.75rem; align-items: center; flex-wrap: wrap; }
.stage { display: grid; gap: 0.3rem; margin-top: 0.25rem; }
.full { display: grid; gap: 0.2rem; }
.full p { margin: 0; }
</style>
