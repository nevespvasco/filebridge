<script setup lang="ts">
import { computed } from "vue";
import FileDropZone from "@/components/FileDropZone.vue";
import DetailsPanel from "@/components/DetailsPanel.vue";
import { useFmt } from "@/composables/useFmt";
import { useJobs } from "@/stores/jobs";
import { useWizard } from "@/stores/wizard";

const wizard = useWizard();
const jobs = useJobs();
const fmt = useFmt();
const scan = computed(() => (wizard.jobId ? jobs.live[wizard.jobId]?.scan : undefined));
</script>

<template>
  <div class="step">
    <h2>{{ $t("source.title") }}</h2>
    <p class="muted">{{ $t("source.intro") }}</p>
    <FileDropZone @add="wizard.addSources" />

    <div v-if="wizard.sources.length" class="card list">
      <div class="list-head">
        <strong>{{ $t("source.chosen", { n: wizard.sources.length }, wizard.sources.length) }}</strong>
        <span v-if="scan" class="small muted">
          {{ $t("source.total", { files: fmt.count(scan.files), size: fmt.bytes(scan.bytes) }) }}
        </span>
      </div>
      <ul>
        <li v-for="p in wizard.sources" :key="p">
          <span class="ellipsis mono small" :title="p">{{ p }}</span>
          <button class="btn btn-small" :aria-label="$t('source.remove', { path: p })" @click="wizard.removeSource(p)">✕</button>
        </li>
      </ul>
      <p class="small muted">{{ $t("source.countLater") }}</p>
    </div>

    <p class="small muted">{{ $t("source.readOnly") }}</p>
    <DetailsPanel :lines="[]" :note="$t('details.sourceNote')" />

    <div class="footer">
      <span class="grow" />
      <button class="btn btn-primary" :disabled="!wizard.sources.length" @click="wizard.step = 2">{{ $t("common.next") }} →</button>
    </div>
  </div>
</template>

<style scoped>
.list { margin-top: 1rem; }
.list-head { display: flex; gap: 1rem; align-items: baseline; flex-wrap: wrap; }
ul { list-style: none; padding: 0; margin: 0.5rem 0; }
li { display: flex; gap: 0.5rem; align-items: center; justify-content: space-between; padding: 0.3rem 0; border-bottom: 1px solid var(--border); }
</style>
