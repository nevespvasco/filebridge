<script setup lang="ts">
// Export a finished transfer's report: the proof of the migration, as CSV or JSON
// (docs/design/specs/D09-report-export.md). The service writes the file; the window only picks
// where.
import { computed, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { EntryFacets, ExportResult, ReportFormat, ReportScope } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { api, errorMessage } from "@/lib/api";
import { jobLabel } from "@/lib/jobs";
import { canReveal, pickSavePath, revealInFileManager } from "@/lib/platform";
import { useJobs } from "@/stores/jobs";
import AppIcon from "./AppIcon.vue";
import ProgressBar from "./ProgressBar.vue";

const props = defineProps<{ jobId: string | null }>();
const emit = defineEmits<{ close: [] }>();
const jobs = useJobs();
const { t } = useI18n();
const fmt = useFmt();

const format = ref<ReportFormat>("csv");
const scope = ref<ReportScope>("all");
const dirs = ref(false);
const path = ref<string | null>(null);
const busy = ref(false);
const error = ref<string | null>(null);
const done = ref<ExportResult | null>(null);
const facets = shallowRef<EntryFacets | null>(null);

const job = computed(() => (props.jobId ? jobs.byId[props.jobId] : undefined));
const progress = computed(() => (props.jobId ? jobs.live[props.jobId]?.report : undefined));

watch(
  () => props.jobId,
  async (id) => {
    error.value = null;
    done.value = null;
    path.value = null;
    facets.value = null;
    if (!id) return;
    try {
      facets.value = await api.entryFacets(id);
    } catch {
      // The choices just show no counts.
    }
  },
  { immediate: true },
);

/** "photos-2019_2026-10-22.csv": the name in plain ASCII, and the finish date. */
const suggested = computed(() => {
  if (!job.value) return `report.${format.value}`;
  const slug =
    jobLabel(job.value)
      .normalize("NFKD")
      .replace(/[̀-ͯ]/g, "")
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "") || "report";
  const date = new Date(job.value.finished_at ?? job.value.created_at).toISOString().slice(0, 10);
  return `${slug}_${date}.${format.value}`;
});
watch(format, () => {
  if (path.value) path.value = path.value.replace(/\.(csv|json)$/i, `.${format.value}`);
});

async function choose() {
  const p = await pickSavePath(suggested.value, format.value, format.value.toUpperCase());
  if (p) path.value = p;
}

async function run() {
  if (!props.jobId) return;
  if (!path.value) await choose();
  if (!path.value) return;
  busy.value = true;
  error.value = null;
  try {
    done.value = await api.exportReport(props.jobId, { format: format.value, scope: scope.value, include_dirs: dirs.value, path: path.value });
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}
const counts = computed(() => ({
  all: facets.value?.by_status.reduce((n, s) => n + s.files, 0),
  problems: facets.value?.problems,
  renamed: facets.value?.renamed,
}));
const scopes: ReportScope[] = ["all", "problems", "renamed"];
void t;
</script>

<template>
  <Teleport to="body">
    <div v-if="jobId" class="overlay" @click.self="emit('close')" @keydown.esc="emit('close')">
      <div class="dialog card" role="dialog" aria-modal="true" :aria-label="$t('report.title')">
        <h2>{{ $t("report.title") }}<span v-if="job" class="muted">: {{ jobLabel(job) }}</span></h2>
        <template v-if="!done">
          <fieldset>
            <legend class="small muted">{{ $t("report.format") }}</legend>
            <label class="opt"><input v-model="format" type="radio" value="csv" /> {{ $t("report.csv") }}</label>
            <label class="opt"><input v-model="format" type="radio" value="json" /> {{ $t("report.json") }}</label>
          </fieldset>
          <fieldset>
            <legend class="small muted">{{ $t("report.include") }}</legend>
            <label v-for="s in scopes" :key="s" class="opt">
              <input v-model="scope" type="radio" :value="s" />
              {{ $t(`report.scope.${s}`, { n: fmt.count(counts[s] ?? 0) }) }}
            </label>
            <label class="opt"><input v-model="dirs" type="checkbox" /> {{ $t("report.dirs") }}</label>
          </fieldset>
          <div class="where">
            <span class="small muted">{{ $t("report.savedAs") }}</span>
            <span class="path small">{{ path ?? suggested }}</span>
            <button class="btn btn-small" :disabled="busy" @click="choose">{{ $t("common.browse") }}</button>
          </div>
          <p class="small muted">{{ $t("report.private") }}</p>
          <div v-if="busy" class="progress" aria-live="polite">
            <ProgressBar :value="progress?.total ? fmt.percent(progress.rows, progress.total) : 0" :indeterminate="!progress?.total" />
            <span class="small muted num">{{ $t("report.rows", { n: fmt.count(progress?.rows ?? 0) }) }}</span>
          </div>
          <p v-if="error" class="error small" role="alert">{{ error }}</p>
          <div class="actions">
            <button class="btn" :disabled="busy" @click="emit('close')">{{ $t("common.cancel") }}</button>
            <button class="btn btn-primary" :disabled="busy" @click="run"><AppIcon name="export" /> {{ $t("report.export") }}</button>
          </div>
        </template>
        <template v-else>
          <p class="ok-text" role="status"><AppIcon name="checkCircle" /> {{ $t("report.saved", { rows: fmt.count(done.rows), size: fmt.bytes(done.bytes) }) }}</p>
          <p class="path small">{{ done.path }}</p>
          <div class="actions">
            <button v-if="canReveal()" class="btn" @click="revealInFileManager(done.path)"><AppIcon name="reveal" /> {{ $t("entry.revealFile") }}</button>
            <button class="btn btn-primary" @click="emit('close')">{{ $t("common.close") }}</button>
          </div>
        </template>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.overlay { position: fixed; inset: 0; background: var(--overlay); display: grid; place-items: center; z-index: 50; padding: 1rem; }
.dialog { width: min(520px, 100%); display: grid; gap: 0.8rem; }
h2 { margin: 0; font-size: 1.1rem; overflow-wrap: anywhere; }
fieldset { border: 0; margin: 0; padding: 0; display: grid; gap: 0.3rem; }
legend { margin-bottom: 0.3rem; }
.opt { display: flex; gap: 0.5rem; align-items: center; cursor: pointer; }
.where { display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; }
.where .path { flex: 1; min-width: 0; }
.progress { display: grid; gap: 0.3rem; }
.actions { display: flex; justify-content: flex-end; gap: 0.5rem; }
p { margin: 0; }
</style>
