<script setup lang="ts">
// The files a job is sending right now, each with its own progress, and the last ones that
// arrived (docs/design/specs/D04-transfer-queue.md).
import { computed } from "vue";
import type { InFlightView_Serialize } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { CATEGORY_ICON, categoryColor } from "@/lib/categories";
import { useJobs } from "@/stores/jobs";
import AppIcon from "./AppIcon.vue";
import ProgressBar from "./ProgressBar.vue";

const props = defineProps<{ jobId: string }>();
const jobs = useJobs();
const fmt = useFmt();

const job = computed(() => jobs.byId[props.jobId]);
const live = computed(() => jobs.live[props.jobId]);
const state = computed(() => job.value?.state);
/** Shown while files move, and greyed while a network error or a pause holds them. */
const shown = computed(() => state.value === "transferring" || state.value === "retrying" || state.value === "paused");
const held = computed(() => state.value !== "transferring");
const rows = computed<InFlightView_Serialize[]>(() => live.value?.transfer?.in_flight ?? []);
const started = computed(() => !!live.value?.transfer);
const arrived = computed(() => live.value?.arrived ?? []);
const lastMinute = computed(() => jobs.arrivedLastMinute(props.jobId));

/** ETA only when it says something (over a minute). */
const eta = (f: InFlightView_Serialize) => (f.eta_secs != null && f.eta_secs > 60 ? fmt.duration(f.eta_secs) : "");
</script>

<template>
  <section v-if="shown" class="queue" :class="{ held }" :aria-labelledby="`queue-${jobId}`">
    <header class="head">
      <h2 :id="`queue-${jobId}`">{{ $t("queue.title", { n: rows.length }) }}</h2>
      <span v-if="live?.transfer && !held" class="small muted num">{{ $t("queue.total", { speed: fmt.rate(live.transfer.bytes_per_sec) }) }}</span>
    </header>

    <p v-if="state === 'paused'" class="small muted">{{ $t("queue.paused") }}</p>
    <template v-if="rows.length">
      <!-- Explicit roles: the narrow form is not displayed as a table (see JobsView). -->
      <table class="table rows" role="table">
        <caption class="sr-only">{{ $t("queue.caption") }}</caption>
        <colgroup>
          <col class="c-name" />
          <col class="c-size" />
          <col class="c-bar" />
          <col class="c-speed" />
        </colgroup>
        <thead class="sr-only" role="rowgroup">
          <tr role="row">
            <th scope="col" role="columnheader">{{ $t("queue.col.name") }}</th>
            <th scope="col" role="columnheader">{{ $t("queue.col.size") }}</th>
            <th scope="col" role="columnheader">{{ $t("queue.col.progress") }}</th>
            <th scope="col" role="columnheader">{{ $t("queue.col.speed") }}</th>
          </tr>
        </thead>
        <tbody role="rowgroup">
          <tr v-for="f in rows" :key="`${f.dir}/${f.name}`" role="row" :style="{ '--cat': categoryColor(f.category) }">
            <td class="name">
              <div class="name-in">
                <AppIcon class="cat" :name="CATEGORY_ICON[f.category]" :style="{ color: categoryColor(f.category) }" />
                <span class="text">
                  <strong class="ellipsis" :title="f.name">{{ f.name }}</strong>
                  <span class="path muted ellipsis" :title="f.dir">{{ f.dir || "/" }}</span>
                </span>
                <AppIcon v-if="f.renamed_from" name="renamed" class="renamed" :label="$t('queue.renamed', { name: f.renamed_from })" />
              </div>
            </td>
            <td class="n small size">{{ fmt.bytes(f.size) }}</td>
            <td class="bar">
              <ProgressBar :value="fmt.percent(f.bytes, f.size)" :tone="held ? 'warn' : 'accent'" />
              <span class="small num pct">{{ Math.round(fmt.percent(f.bytes, f.size)) }} %</span>
            </td>
            <td class="n small muted speed">
              {{ fmt.rate(f.bytes_per_sec) }}<template v-if="eta(f)"> · {{ eta(f) }}</template>
            </td>
          </tr>
        </tbody>
      </table>
    </template>
    <p v-else-if="state === 'transferring'" class="small muted">{{ started ? $t("queue.between") : $t("queue.starting") }}</p>

    <details v-if="arrived.length" class="arrived">
      <summary class="small">
        {{ $t("queue.finished") }}
        <span v-if="lastMinute" class="muted num">· {{ $t("queue.lastMinute", { n: fmt.count(lastMinute) }) }}</span>
      </summary>
      <ul class="done-list small">
        <li v-for="d in arrived" :key="d.idx">
          <AppIcon name="check" class="ok-text" />
          <span class="ellipsis" :title="d.name">{{ d.name }}</span>
        </li>
      </ul>
    </details>
  </section>
</template>

<style scoped>
.queue { display: grid; gap: 0.6rem; container-type: inline-size; }
.head { display: flex; align-items: baseline; justify-content: space-between; gap: 0.25rem 1rem; flex-wrap: wrap; }
h2 { font-size: 1rem; margin: 0; }
.rows td { padding: 0.45rem 0.6rem; border-bottom-color: var(--rule); }
.rows { table-layout: fixed; }
.name { padding-left: 0.75rem !important; box-shadow: inset 3px 0 0 var(--cat); }
.c-name { width: 42%; }
.c-size { width: 12%; }
.c-bar { width: 28%; }
.c-speed { width: 18%; }
.name-in { display: flex; align-items: center; gap: 0.55rem; min-width: 0; }
.cat { flex: none; }
.text { display: grid; min-width: 0; flex: 1; }
.renamed { color: var(--text-2); }
.bar :deep(.progress) { display: inline-block; width: calc(100% - 3.5rem); vertical-align: middle; }
.pct { display: inline-block; width: 3.2rem; text-align: right; }
td.n { white-space: nowrap; }
.held .rows { opacity: 0.6; }
.arrived summary { cursor: pointer; color: var(--text-2); }
.done-list { list-style: none; margin: 0.4rem 0 0; padding: 0; display: flex; flex-wrap: wrap; gap: 0.3rem 1rem; }
.done-list li { display: inline-flex; gap: 0.3rem; align-items: center; max-width: min(16rem, 100%); min-width: 0; }
/* A narrow queue: each file on two lines, its name and size, then its bar and speed. */
@container (max-width: 560px) {
  .rows, .rows tbody { display: block; }
  .rows colgroup { display: none; }
  .rows tr { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 0.3rem 0.75rem; align-items: center; padding: 0.5rem 0.6rem 0.5rem 0.75rem; border-bottom: 1px solid var(--rule); box-shadow: inset 3px 0 0 var(--cat); }
  .rows tr:last-child { border-bottom: 0; }
  .rows td { display: block; padding: 0 !important; border: 0; }
  .rows td.name { box-shadow: none; }
  .rows td.bar { display: flex; align-items: center; gap: 0.5rem; }
  .bar :deep(.progress) { flex: 1; width: auto; }
  .pct { width: auto; min-width: 2.8rem; }
}
</style>
