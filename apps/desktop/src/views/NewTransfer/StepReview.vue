<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { computed, onMounted, ref, watch } from "vue";
import { defaultLabel } from "@/lib/jobs";
import { errorMessage } from "@/lib/api";
import DetailsPanel from "@/components/DetailsPanel.vue";
import HelpTip from "@/components/HelpTip.vue";
import { useFmt } from "@/composables/useFmt";
import { request } from "@/lib/requests";
import { minutesToTime } from "@/lib/format";
import { useJobs } from "@/stores/jobs";
import { sendMode, profileWhere, useProfiles } from "@/stores/profiles";
import { useSettings } from "@/stores/settings";
import { useWizard } from "@/stores/wizard";
import { useConnection } from "@/stores/connection";

const wizard = useWizard();
const conn = useConnection();
const jobs = useJobs();
const profiles = useProfiles();
const settings = useSettings();
const fmt = useFmt();
const sendWindow = computed(() => {
  const w = settings.app.send_window;
  return w && { start: minutesToTime(w.start_minute), end: minutesToTime(w.end_minute) };
});

const profile = computed(() => (wizard.profileId ? profiles.byId[wizard.profileId] : undefined));
const job = computed(() => (wizard.jobId ? jobs.byId[wizard.jobId] : undefined));
const totals = computed(() => wizard.analysis ?? job.value?.counts);
const renames = computed(() => wizard.ruleCounts.reduce((n, r) => n + r.total, 0));
const mode = computed(() => (profile.value ? sendMode(profile.value) : "local"));

onMounted(() => profiles.ensure(wizard.profileId));

// The transfer's name (docs/design/specs/D06): saved when the field is left.
const label = ref(job.value?.label ?? "");
const labelError = ref<string | null>(null);
watch(() => job.value?.label, (l) => (label.value = l ?? ""));
async function saveLabel() {
  if (!wizard.jobId || (label.value.trim() || null) === (job.value?.label ?? null)) return;
  try {
    await jobs.setLabel(wizard.jobId, label.value.trim() || null);
    labelError.value = null;
  } catch (e) {
    labelError.value = errorMessage(e);
  }
}

const details = computed(() =>
  wizard.jobId
    ? [request("set_options", { job_id: wizard.jobId, options: wizard.options }), request("start", { job_id: wizard.jobId }), request("subscribe")]
    : [],
);
</script>

<template>
  <div class="step">
    <h2>{{ $t("review.title") }}</h2>
    <div class="guide">
      <AppIcon class="guide-icon" name="review" :size="20" />
      <p>{{ $t("review.guide") }}</p>
    </div>
    <label class="field label-field">
      <span>{{ $t("review.label") }}</span>
      <input v-model="label" maxlength="120" :placeholder="defaultLabel(wizard.sources)" @blur="saveLabel" @keydown.enter="saveLabel" />
      <span class="hint">{{ $t("review.labelHint") }}</span>
      <span v-if="labelError" class="error small">{{ labelError }}</span>
    </label>
    <dl class="card summary">
      <dt>{{ $t("review.sources") }}</dt>
      <dd>
        <span v-for="s in wizard.sources" :key="s" class="mono small block ellipsis" :title="s">{{ s }}</span>
      </dd>
      <dt>{{ $t("review.destination") }}</dt>
      <dd>
        <strong>{{ profile?.name ?? wizard.profileId }}</strong>
        <span v-if="profile" class="path block">{{ profileWhere(profile) }}</span>
      </dd>
      <dt>{{ $t("review.files") }}</dt>
      <dd>{{ fmt.count(totals?.files) }} · {{ fmt.bytes(totals?.bytes) }}</dd>
      <dt>{{ $t("review.renames") }}<HelpTip :text="$t('review.renamesHelp')" :label="$t('review.renames')" /></dt>
      <dd>{{ $t("review.renamesValue", { n: fmt.count(renames) }, renames) }}</dd>
      <dt>{{ $t("review.mode") }}<HelpTip :text="$t('review.modeHelp')" :label="$t('review.mode')" /></dt>
      <dd>{{ $t(`review.modes.${mode}`) }}</dd>
    </dl>

    <label class="check card">
      <input v-model="wizard.options.verify" type="checkbox" @change="wizard.changeOptions()" />
      <span>
        <strong>{{ $t("review.verify") }}</strong> <span class="badge badge-ok">{{ $t("common.recommended") }}</span>
        <span class="small muted block">{{ $t("review.verifyHint") }}</span>
      </span>
    </label>

    <p v-if="sendWindow" class="guide small"><AppIcon class="guide-icon" name="night" :size="20" /><span>{{ $t("review.window", sendWindow) }}</span></p>
    <p v-if="settings.app.bandwidth_limit" class="guide small">
      <AppIcon class="guide-icon" name="slow" :size="20" /><span>{{ $t("review.limit", { rate: fmt.rate(settings.app.bandwidth_limit) }) }}</span>
    </p>
    <p class="guide ok small"><AppIcon class="guide-icon" name="lock" :size="20" /><span>{{ $t("review.safety") }}</span></p>
    <p v-if="wizard.sendError" class="error">{{ wizard.sendError }}</p>

    <DetailsPanel :lines="details" />

    <div class="footer">
      <button class="btn" @click="wizard.step = 3"><AppIcon name="back" /> {{ $t("common.back") }}</button>
      <span class="grow" />
      <span v-if="conn.offline" class="why-disabled">{{ $t("conn.disconnected") }}</span>
      <button class="btn btn-primary btn-big" :title="$t('review.sendTip')" :disabled="wizard.sending || !wizard.canReview || conn.offline" @click="wizard.send()">
        {{ wizard.sending ? $t("review.starting") : job?.counts.bytes ? $t("review.sendSize", { size: fmt.bytes(job.counts.bytes) }) : $t("review.send") }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.label-field { max-width: 32rem; margin-bottom: 1rem; }
.summary { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 0.5rem 1.5rem; align-items: start; margin: 0 0 1rem; }
dt { color: var(--text-2); display: flex; align-items: center; }
dd { margin: 0; min-width: 0; }
.block { display: block; }
.check { display: flex; gap: 0.75rem; align-items: flex-start; cursor: pointer; margin-bottom: 1rem; }
.check input { margin-top: 0.3rem; }
/* A phone: each value under its label, with the whole width (paths, the mode's sentence). */
@media (max-width: 559px) {
  .summary { grid-template-columns: minmax(0, 1fr); gap: 0.1rem; }
  .summary dd + dt { margin-top: 0.5rem; }
  dt { font-size: var(--fs-sm); }
}
</style>
