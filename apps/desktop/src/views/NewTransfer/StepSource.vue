<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
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

    <div v-if="!wizard.sources.length" class="guide how">
      <AppIcon class="guide-icon" name="lightbulb" :size="20" />
      <div>
        <p><strong>{{ $t("source.howTitle") }}</strong></p>
        <ol>
          <li v-for="n in 4" :key="n">{{ $t(`source.how.${n}`) }}</li>
        </ol>
      </div>
    </div>

    <p class="lead-text">{{ $t("source.intro") }} <span class="examples">{{ $t("source.examples") }}</span></p>
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
          <button class="btn btn-small" :title="$t('source.removeTip')" :aria-label="$t('source.remove', { path: p })" @click="wizard.removeSource(p)"><AppIcon name="close" /></button>
        </li>
      </ul>
      <p class="small muted">{{ $t("source.addMore") }} {{ $t("source.countLater") }}</p>
    </div>

    <p class="guide ok small safe"><AppIcon class="guide-icon" name="lock" :size="20" /><span>{{ $t("source.readOnly") }}</span></p>
    <DetailsPanel :lines="[]" :note="$t('details.sourceNote')" />

    <div class="footer">
      <span class="grow" />
      <span v-if="!wizard.sources.length" class="why-disabled">{{ $t("source.needOne") }}</span>
      <button class="btn btn-primary" :disabled="!wizard.sources.length" @click="wizard.step = 2">{{ $t("common.next") }} <AppIcon name="next" /></button>
    </div>
  </div>
</template>

<style scoped>
.list { margin: 1rem 0; }
.how { margin-bottom: 1.25rem; }
.safe { margin-top: 1rem; }
.examples { display: block; font-size: 0.85rem; margin-top: 0.2rem; }
.list-head { display: flex; gap: 1rem; align-items: baseline; flex-wrap: wrap; }
.list ul { list-style: none; padding: 0; margin: 0.5rem 0; }
.list li { display: flex; gap: 0.5rem; align-items: center; justify-content: space-between; padding: 0.3rem 0; border-bottom: 1px solid var(--border); }
</style>
