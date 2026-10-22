<script setup lang="ts">
import { computed, onMounted } from "vue";
import DetailsPanel from "@/components/DetailsPanel.vue";
import HelpTip from "@/components/HelpTip.vue";
import { useFmt } from "@/composables/useFmt";
import { request } from "@/lib/requests";
import { useJobs } from "@/stores/jobs";
import { profileKind, profileWhere, useProfiles } from "@/stores/profiles";
import { useWizard } from "@/stores/wizard";

const wizard = useWizard();
const jobs = useJobs();
const profiles = useProfiles();
const fmt = useFmt();

const profile = computed(() => (wizard.profileId ? profiles.byId[wizard.profileId] : undefined));
const job = computed(() => (wizard.jobId ? jobs.byId[wizard.jobId] : undefined));
const totals = computed(() => wizard.analysis ?? job.value?.counts);
const renames = computed(() => wizard.ruleCounts.reduce((n, r) => n + r.total, 0));
const mode = computed(() => (profile.value ? profileKind(profile.value) : "local"));

onMounted(() => profiles.ensure(wizard.profileId));

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
      <span class="guide-icon" aria-hidden="true">👀</span>
      <p>{{ $t("review.guide") }}</p>
    </div>
    <dl class="card summary">
      <dt>{{ $t("review.sources") }}</dt>
      <dd>
        <span v-for="s in wizard.sources" :key="s" class="mono small block ellipsis" :title="s">{{ s }}</span>
      </dd>
      <dt>{{ $t("review.destination") }}</dt>
      <dd>
        <strong>{{ profile?.name ?? wizard.profileId }}</strong>
        <span v-if="profile" class="mono small block">{{ profileWhere(profile) }}</span>
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

    <p class="guide ok small"><span class="guide-icon" aria-hidden="true">🔒</span><span>{{ $t("review.safety") }}</span></p>
    <p v-if="wizard.sendError" class="error">{{ wizard.sendError }}</p>

    <DetailsPanel :lines="details" />

    <div class="footer">
      <button class="btn" @click="wizard.step = 3">← {{ $t("common.back") }}</button>
      <span class="grow" />
      <button class="btn btn-primary btn-big" :title="$t('review.sendTip')" :disabled="wizard.sending || !wizard.canReview" @click="wizard.send()">
        {{ wizard.sending ? $t("review.starting") : $t("review.send") }}
      </button>
    </div>
  </div>
</template>

<style scoped>
.summary { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 0.5rem 1.5rem; margin: 0 0 1rem; }
dt { color: var(--text-2); display: flex; align-items: center; }
dd { margin: 0; min-width: 0; }
.block { display: block; }
.check { display: flex; gap: 0.75rem; align-items: flex-start; cursor: pointer; margin-bottom: 1rem; }
.check input { margin-top: 0.3rem; }
</style>
