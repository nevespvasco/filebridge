<script setup lang="ts">
// How a finished transfer ended, in one sentence and one big number, with what to do next
// (docs/design/01-design-system.md, "Result card"). The numbers are the Files tab's own (the
// entries' facets), so the card and the list never disagree; green only for "verified, no
// differences".
import { computed, shallowRef, watch } from "vue";
import { useRouter } from "vue-router";
import { JOB_OPERATIONS, type EntryFacets, type EntryView, type JobState } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { api } from "@/lib/api";
import { TERMINAL, useJobs } from "@/stores/jobs";
import { useReclaim } from "@/stores/reclaim";
import AppIcon from "./AppIcon.vue";
import type { IconName } from "@/lib/icons";
import { i18n } from "@/i18n";

const props = defineProps<{ jobId: string }>();
const emit = defineEmits<{ exportReport: [] }>();
const jobs = useJobs();
const reclaim = useReclaim();
const router = useRouter();
const fmt = useFmt();

const job = computed(() => jobs.byId[props.jobId]);
const done = computed(() => !!job.value && TERMINAL.includes(job.value.state));
const facets = shallowRef<EntryFacets | null>(null);
/** The first files that couldn't be sent, with why (the engine gave up on them after its
 *  retries, ADR 0033); the rest are counted. */
const notSent = shallowRef<EntryView[]>([]);
const NOT_SENT_LISTED = 10;
watch(
  () => [props.jobId, done.value, job.value?.finished_at] as const,
  async ([id, finished]) => {
    facets.value = null;
    notSent.value = [];
    if (!finished) return;
    try {
      facets.value = await api.entryFacets(id);
    } catch {
      // The job's own counts stand in.
    }
    if (!failedCount()) return;
    try {
      const page = await api.entries(id, { statuses: ["failed"], categories: [], renamed: null, include_dirs: false, query: null, sort: "folder_order" }, null, NOT_SENT_LISTED);
      notSent.value = page.items;
    } catch {
      // "See the problems" still lists them.
    }
  },
  { immediate: true },
);
const notSentMore = computed(() => Math.max(0, failedCount() - notSent.value.length));

function byStatus(s: string) {
  return facets.value?.by_status.find((x) => x.status === s)?.files ?? 0;
}
function failedCount() {
  return facets.value ? byStatus("failed") : (job.value?.counts.failed ?? 0);
}
const n = computed(() => {
  const j = job.value;
  if (!j) return { files: 0, copied: 0, verified: 0, problems: 0, skipped: 0, renamed: 0 };
  if (!facets.value) {
    return { files: j.counts.files, copied: j.counts.done, verified: j.counts.verified_ok, problems: j.counts.failed + j.counts.mismatch, skipped: j.counts.skipped, renamed: 0 };
  }
  const files = facets.value.by_status.reduce((sum, s) => sum + s.files, 0);
  return {
    files,
    copied: files - byStatus("not_sent") - byStatus("sending") - byStatus("failed") - byStatus("skipped"),
    verified: byStatus("verified"),
    problems: facets.value.problems,
    skipped: byStatus("skipped"),
    renamed: facets.value.renamed,
  };
});

type Tone = "ok" | "accent" | "warn" | "danger" | "muted";
/** The big number (all files when everything arrived, else those copied), the sentence after it
 *  and the one under it. */
const view = computed<{ tone: Tone; icon: IconName; stat: number; title: string; sub: string }>(() => {
  const j = job.value!;
  const c = n.value;
  const t = i18n.global.t;
  const of = { n: fmt.count(c.files), problems: fmt.count(c.problems) };
  if (j.state === "cancelled") return { tone: "muted", icon: "close", stat: c.copied, title: t("result.cancelled", of, c.files), sub: t("result.cancelledSub") };
  if (j.state === "failed") return { tone: "danger", icon: "failCircle", stat: c.copied, title: t("result.failed", of, c.files), sub: j.error?.split("\n")[0] ?? "" };
  if (c.problems > 0) return { tone: "warn", icon: "warning", stat: c.copied, title: t("result.problems", of, c.files), sub: t("result.problemsSub", of, c.problems) };
  if (j.options.verify) return { tone: "ok", icon: "shield", stat: c.files, title: t("result.verified", of, c.files), sub: t("result.noDifferences") };
  return { tone: "accent", icon: "checkCircle", stat: c.files, title: t("result.copied", of, c.files), sub: t("result.notVerified") };
});

const can = (op: keyof typeof JOB_OPERATIONS, state: JobState) => (JOB_OPERATIONS[op] as readonly JobState[]).includes(state);
const canExport = computed(() => !!job.value && can("export_report", job.value.state));
const reclaimable = computed(() => reclaim.eligible.find((s) => s.job_id === props.jobId));

function seeProblems() {
  void router.push({ name: "job", params: { id: props.jobId }, query: { tab: "files", status: "problems" } });
}
function seeReclaim() {
  document.getElementById("reclaim")?.scrollIntoView({ block: "start" });
}
</script>

<template>
  <section v-if="job && done" class="result card" :class="`tone-${view.tone}`" aria-labelledby="result-title">
    <AppIcon :name="view.icon" :size="36" class="result-icon" />
    <div class="body">
      <h2 id="result-title" class="title">
        <span class="stat num">{{ fmt.count(view.stat) }}</span>
        {{ view.title }}
      </h2>
      <p v-if="view.sub" class="sub">{{ view.sub }}</p>
      <ul class="facts small muted">
        <li class="num">{{ fmt.bytes(job.counts.bytes) }}</li>
        <li v-if="n.renamed" class="num">{{ $t("result.renamed", { n: fmt.count(n.renamed) }, n.renamed) }}</li>
        <li v-if="n.skipped" class="num">{{ $t("result.skipped", { n: fmt.count(n.skipped) }, n.skipped) }}</li>
        <li v-if="job.attempt" class="num">{{ $t("result.retries", { n: job.attempt }, job.attempt) }}</li>
        <li v-if="job.finished_at"><span>{{ fmt.date(job.finished_at) }}</span></li>
      </ul>
      <div v-if="notSent.length" class="not-sent">
        <p class="small">{{ $t("result.notSent") }}</p>
        <ul class="small">
          <li v-for="e in notSent" :key="e.idx">
            <span class="name">{{ e.dir ? `${e.dir}/${e.name}` : e.name }}</span>
            <span v-if="e.error" class="muted"> — {{ e.error }}</span>
          </li>
          <li v-if="notSentMore" class="muted">{{ $t("result.notSentMore", { n: fmt.count(notSentMore) }) }}</li>
        </ul>
      </div>
      <p v-if="reclaimable" class="small reclaim-line"><AppIcon name="broom" /> {{ $t("result.reclaim", { size: fmt.bytes(reclaimable.bytes) }) }}</p>
      <div class="actions">
        <button v-if="n.problems > 0" class="btn btn-primary" @click="seeProblems"><AppIcon name="warnCircle" /> {{ $t("result.seeProblems") }}</button>
        <button v-if="canExport" class="btn" :class="{ 'btn-primary': view.tone === 'ok' }" @click="emit('exportReport')"><AppIcon name="export" /> {{ $t("result.export") }}</button>
        <button v-if="reclaimable" class="btn" @click="seeReclaim"><AppIcon name="broom" /> {{ $t("result.seeReclaim") }}</button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.result { display: flex; gap: var(--sp-4); align-items: flex-start; padding: var(--sp-5); border-width: 1px; border-radius: var(--radius-lg); }
.result-icon { flex: none; }
.body { display: grid; gap: var(--sp-2); min-width: 0; flex: 1; }
.title { margin: 0; font-size: var(--fs-lg); font-weight: 600; line-height: 1.3; }
.stat { display: block; font-size: var(--fs-stat); font-weight: 700; line-height: 1.1; }
.sub { margin: 0; font-size: var(--fs-base); }
/* Every fact has its "·" in front; the list starts one dot-width to the left and is clipped there,
   so a line never starts with a dot, however the facts wrap. */
.facts { list-style: none; margin: 0 0 0 calc(-1 * var(--sp-5)); padding: 0; display: flex; flex-wrap: wrap; gap: var(--sp-1) 0; clip-path: inset(0 0 0 var(--sp-5)); }
.facts li { padding-left: var(--sp-5); }
.facts li::before { content: "·"; display: inline-block; width: var(--sp-5); margin-left: calc(-1 * var(--sp-5)); text-align: center; color: var(--muted); }
.not-sent p { margin: 0; }
.not-sent ul { margin: var(--sp-1) 0 0; padding-left: var(--sp-5); display: grid; gap: var(--sp-1); }
.not-sent .name { overflow-wrap: anywhere; }
.reclaim-line { margin: 0; display: flex; gap: var(--sp-2); align-items: center; color: var(--ok); }
.actions { display: flex; gap: var(--sp-2); flex-wrap: wrap; margin-top: var(--sp-1); }
.tone-ok { background: var(--ok-soft); border-color: color-mix(in srgb, var(--ok) 45%, transparent); }
.tone-ok .result-icon { color: var(--ok); }
.tone-accent .result-icon { color: var(--accent); }
.tone-warn { background: var(--warn-soft); border-color: color-mix(in srgb, var(--warn) 45%, transparent); }
.tone-warn .result-icon { color: var(--warn-strong); }
.tone-danger { background: var(--danger-soft); border-color: color-mix(in srgb, var(--danger) 45%, transparent); }
.tone-danger .result-icon { color: var(--danger); }
.tone-muted .result-icon { color: var(--text-2); }
@media (max-width: 559px) {
  .result { flex-direction: column; gap: var(--sp-2); padding: var(--sp-4); }
}
</style>
