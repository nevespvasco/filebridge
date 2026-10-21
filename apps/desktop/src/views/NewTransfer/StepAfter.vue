<script setup lang="ts">
import { computed } from "vue";
import { useRouter } from "vue-router";
import JobProgress from "@/components/JobProgress.vue";
import DetailsPanel from "@/components/DetailsPanel.vue";
import { useFmt } from "@/composables/useFmt";
import { request } from "@/lib/requests";
import { TERMINAL, useJobs } from "@/stores/jobs";
import { useWizard } from "@/stores/wizard";

const wizard = useWizard();
const jobs = useJobs();
const router = useRouter();
const fmt = useFmt();

const id = computed(() => wizard.jobId!);
const job = computed(() => jobs.byId[id.value]);
const live = computed(() => jobs.live[id.value]);
const done = computed(() => !!job.value && TERMINAL.includes(job.value.state));

function newTransfer() {
  wizard.reset();
}
</script>

<template>
  <div class="step">
    <h2>{{ done ? $t("after.reportTitle") : $t("after.title") }}</h2>
    <p v-if="!done" class="muted">{{ $t("after.closeOk") }}</p>

    <div class="card"><JobProgress :job-id="id" /></div>

    <div v-if="done && job" class="card report">
      <h3>{{ $t(`after.result.${job.state}`) }}</h3>
      <div class="stats">
        <div><span class="big">{{ fmt.count(job.counts.done) }}</span><span class="small muted">{{ $t("counts.done") }}</span></div>
        <div><span class="big">{{ fmt.count(job.counts.failed) }}</span><span class="small muted">{{ $t("counts.failed") }}</span></div>
        <div><span class="big">{{ fmt.count(job.counts.skipped) }}</span><span class="small muted">{{ $t("counts.skipped") }}</span></div>
        <div v-if="job.options.verify"><span class="big">{{ fmt.count(job.counts.verified_ok) }}</span><span class="small muted">{{ $t("counts.verified") }}</span></div>
        <div v-if="job.options.verify"><span class="big">{{ fmt.count(job.counts.mismatch) }}</span><span class="small muted">{{ $t("counts.mismatch") }}</span></div>
      </div>
      <p v-if="job.options.verify && job.counts.mismatch === 0 && job.state === 'completed'" class="ok-text small">{{ $t("after.verifiedAll") }}</p>
      <details v-if="live?.failures.length" class="list">
        <summary class="small warn-text">{{ $t("after.failures", { n: live.failures.length }, live.failures.length) }}</summary>
        <ul class="small mono">
          <li v-for="(f, i) in live.failures" :key="i">{{ f.path }} — {{ f.error }}</li>
        </ul>
      </details>
      <details v-if="live?.problems.length" class="list" open>
        <summary class="small warn-text">{{ $t("after.problems", { n: live.problems.length }, live.problems.length) }}</summary>
        <ul class="small">
          <li v-for="(p, i) in live.problems" :key="i"><code>{{ p.code }}</code> {{ p.path ? `${p.path}: ` : "" }}{{ p.message }}</li>
        </ul>
      </details>
    </div>

    <DetailsPanel :lines="[request('job', { job_id: id })]" />

    <div class="footer">
      <button class="btn" @click="router.push({ name: 'job', params: { id } })">{{ $t("after.openJob") }}</button>
      <span class="grow" />
      <button class="btn btn-primary" @click="newTransfer">{{ $t("after.newTransfer") }}</button>
    </div>
  </div>
</template>

<style scoped>
.report { margin-top: 1rem; }
.report h3 { margin-top: 0; }
.stats { display: flex; gap: 2rem; flex-wrap: wrap; }
.stats > div { display: grid; }
.big { font-size: 1.4rem; font-weight: 600; }
.list { margin-top: 0.75rem; }
.list ul { max-height: 14rem; overflow: auto; }
</style>
