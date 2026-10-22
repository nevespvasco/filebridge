<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { computed, onActivated, onBeforeUnmount, onMounted, ref } from "vue";
import type { RecentSource } from "@/bindings";
import RelativeTime from "@/components/RelativeTime.vue";
import { api } from "@/lib/api";
import { baseName } from "@/lib/format";
import { useProfiles } from "@/stores/profiles";
import FileDropZone from "@/components/FileDropZone.vue";
import DetailsPanel from "@/components/DetailsPanel.vue";
import { useFmt } from "@/composables/useFmt";
import { useJobs } from "@/stores/jobs";
import { useWizard } from "@/stores/wizard";

const wizard = useWizard();
const jobs = useJobs();
const fmt = useFmt();
const scan = computed(() => (wizard.jobId ? jobs.live[wizard.jobId]?.scan : undefined));
const profiles = useProfiles();

// "Recently sent from" (docs/design/specs/D02): re-read when the window comes back, since a
// disk may have been plugged in or out meanwhile.
const recent = ref<RecentSource[]>([]);
async function loadRecent() {
  try {
    recent.value = await api.recentSources(12);
  } catch {
    recent.value = [];
  }
}
const recentShown = computed(() => recent.value.filter((r) => !wizard.sources.includes(r.path)).slice(0, 6));
onMounted(loadRecent);
onActivated(loadRecent);
window.addEventListener("focus", loadRecent);
onBeforeUnmount(() => window.removeEventListener("focus", loadRecent));
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

    <section v-if="recentShown.length" class="recent" :aria-label="$t('source.recent')">
      <h3 class="small muted">{{ $t("source.recent") }}</h3>
      <div class="chips">
        <button
          v-for="r in recentShown"
          :key="r.path"
          class="chip"
          :aria-disabled="!r.exists"
          :title="r.exists ? r.path : $t('source.recentMissing', { path: r.path })"
          @click="r.exists && wizard.addSources([r.path])"
        >
          <AppIcon name="folder" />
          <span class="name">{{ baseName(r.path) }}</span>
          <span class="muted small">· {{ $t("source.recentTo", { destination: profiles.byId[r.last_profile_id]?.name ?? "—" }) }} · <RelativeTime :at="r.last_sent_at" /></span>
        </button>
      </div>
    </section>

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
.recent { margin: 1rem 0; }
.recent h3 { margin: 0 0 0.4rem; font-weight: 600; }
.chips { display: flex; flex-wrap: wrap; gap: 0.5rem; }
.chip { font: inherit; display: inline-flex; gap: 0.4rem; align-items: center; max-width: 100%; padding: 0.35rem 0.7rem; border: 1px solid var(--border-strong); border-radius: 999px; background: var(--surface); color: var(--text); cursor: pointer; }
.chip:hover:not([aria-disabled="true"]) { background: var(--surface-2); }
.chip:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.chip[aria-disabled="true"] { opacity: 0.5; cursor: not-allowed; }
.chip .name { font-weight: 600; }
.how { margin-bottom: 1.25rem; }
.safe { margin-top: 1rem; }
.examples { display: block; font-size: 0.85rem; margin-top: 0.2rem; }
.list-head { display: flex; gap: 1rem; align-items: baseline; flex-wrap: wrap; }
.list ul { list-style: none; padding: 0; margin: 0.5rem 0; }
.list li { display: flex; gap: 0.5rem; align-items: center; justify-content: space-between; padding: 0.3rem 0; border-bottom: 1px solid var(--border); }
</style>
