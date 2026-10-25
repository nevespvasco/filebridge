<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import JobProgress from "@/components/JobProgress.vue";
import ReportDialog from "@/components/ReportDialog.vue";
import ResultCard from "@/components/ResultCard.vue";
import TransferQueue from "@/components/TransferQueue.vue";
import { TERMINAL, useJobs } from "@/stores/jobs";
import { useWizard } from "@/stores/wizard";

const wizard = useWizard();
const jobs = useJobs();
const router = useRouter();

const id = computed(() => wizard.jobId!);
const job = computed(() => jobs.byId[id.value]);
const live = computed(() => jobs.live[id.value]);
const done = computed(() => !!job.value && TERMINAL.includes(job.value.state));
const reportOpen = ref(false);

function newTransfer() {
  wizard.reset();
}
</script>

<template>
  <div class="step">
    <h2>{{ done ? $t("after.reportTitle") : $t("after.title") }}</h2>
    <div v-if="!done" class="guide">
      <AppIcon class="guide-icon" name="coffee" :size="20" />
      <p>{{ $t("after.closeOk") }}</p>
    </div>

    <ResultCard v-if="done" :job-id="id" @export-report="reportOpen = true" />
    <div v-else class="card"><JobProgress :job-id="id" /></div>
    <div v-if="job && ['transferring', 'retrying', 'paused'].includes(job.state)" class="card queue-card"><TransferQueue :job-id="id" /></div>

    <div v-if="done && job && (live?.failures.length || live?.problems.length)" class="card report">
      <details v-if="live?.failures.length" class="list">
        <summary class="small warn-text">{{ $t("after.failures", { n: live.failures.length }, live.failures.length) }}</summary>
        <p class="small muted">{{ $t("after.failuresHint") }}</p>
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
    <ReportDialog :job-id="reportOpen ? id : null" @close="reportOpen = false" />

    <div class="footer">
      <button class="btn" :title="$t('after.openJobTip')" @click="router.push({ name: 'job', params: { id } })">{{ $t("after.openJob") }}</button>
      <span class="grow" />
      <button class="btn btn-primary" @click="newTransfer">{{ $t("after.newTransfer") }}</button>
    </div>
  </div>
</template>

<style scoped>
.queue-card { margin-top: 1rem; }
.report { margin-top: 1rem; }
.list + .list { margin-top: 0.75rem; }
.list ul { max-height: 14rem; overflow: auto; }
</style>
