<script setup lang="ts">
// The files of a transfer, from its manifest: filter, search, sort, and open one for its detail
// (docs/design/specs/D05-files-and-entry-detail.md). Paged from the service and virtualised, so
// 300 000 files never sit in the window at once.
import { computed, onBeforeUnmount, ref, shallowRef, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import { useVirtualizer } from "@tanstack/vue-virtual";
import type { EntryCursor, EntryFacets, EntryFilter, EntrySort, EntryStatus, EntryView_Serialize, FileCategory } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { api, errorMessage } from "@/lib/api";
import { CATEGORY_ICON, categoryColor } from "@/lib/categories";
import { PROBLEM_STATUSES } from "@/lib/entries";
import { TERMINAL, useJobs } from "@/stores/jobs";
import AppIcon from "./AppIcon.vue";
import EntryDrawer from "./EntryDrawer.vue";
import EntryStatusBadge from "./EntryStatusBadge.vue";
import FilterChips from "./FilterChips.vue";
import SearchField from "./SearchField.vue";

const props = defineProps<{ jobId: string }>();
const emit = defineEmits<{ exportReport: [] }>();
const route = useRoute();
const router = useRouter();
const jobs = useJobs();
const { t } = useI18n();
const fmt = useFmt();

const PAGE = 200;
const ROW = 46;

// ---- filters, kept in the route query (never the search text: it may be a person's name) ----
type Chip = "problems" | "verified" | "sent" | "renamed" | "skipped" | "not_sent";
const CHIP_STATUSES: Record<Exclude<Chip, "renamed">, EntryStatus[]> = {
  problems: PROBLEM_STATUSES,
  verified: ["verified"],
  sent: ["sent"],
  skipped: ["skipped"],
  not_sent: ["not_sent", "sending"],
};
const list = (v: unknown) => (typeof v === "string" && v ? v.split(",") : []);
const chips = computed<Chip[]>({
  get: () => list(route.query.status) as Chip[],
  set: (v) => setQuery({ status: v.join(",") || undefined }),
});
const categories = computed<FileCategory[]>({
  get: () => list(route.query.cat) as FileCategory[],
  set: (v) => setQuery({ cat: v.join(",") || undefined }),
});
const sort = computed<EntrySort>({
  get: () => (["largest_first", "problems_first"].includes(route.query.sort as string) ? (route.query.sort as EntrySort) : "folder_order"),
  set: (v) => setQuery({ sort: v === "folder_order" ? undefined : v }),
});
const search = ref("");
function setQuery(patch: Record<string, string | undefined>) {
  void router.replace({ query: { ...route.query, ...patch } });
}

const filter = computed<EntryFilter>(() => ({
  statuses: chips.value.flatMap((c) => (c === "renamed" ? [] : CHIP_STATUSES[c])),
  categories: categories.value,
  renamed: chips.value.includes("renamed") ? true : null,
  include_dirs: false,
  query: search.value.trim() ? search.value.trim().normalize("NFC") : null,
  sort: sort.value,
}));

// ---- data ----
const facets = shallowRef<EntryFacets | null>(null);
const rows = shallowRef<EntryView_Serialize[]>([]);
const next = ref<EntryCursor | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
let generation = 0;

async function loadFacets() {
  try {
    facets.value = await api.entryFacets(props.jobId);
  } catch {
    // The chips just show no counts.
  }
}
async function reload() {
  const gen = ++generation;
  loading.value = true;
  try {
    const page = await api.entries(props.jobId, filter.value, null, PAGE);
    if (gen !== generation) return;
    rows.value = page.items;
    next.value = page.next;
    error.value = null;
  } catch (e) {
    if (gen === generation) error.value = errorMessage(e);
  } finally {
    if (gen === generation) loading.value = false;
  }
}
async function loadMore() {
  if (!next.value || loading.value) return;
  const gen = generation;
  loading.value = true;
  try {
    const page = await api.entries(props.jobId, filter.value, next.value, PAGE);
    if (gen !== generation) return;
    rows.value = [...rows.value, ...page.items];
    next.value = page.next;
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    loading.value = false;
  }
}
watch(() => props.jobId, () => void loadFacets(), { immediate: true });
watch(filter, () => void reload(), { immediate: true, deep: true });

// While the job runs, the statuses change: re-read the counts, and the first page when that is
// all that is loaded (deeper pages stay put so rows don't jump), at most every 2 s.
const job = computed(() => jobs.byId[props.jobId]);
let liveTimer: ReturnType<typeof setTimeout> | undefined;
watch(
  () => [job.value?.state, job.value?.counts.done, job.value?.counts.verified_ok, jobs.live[props.jobId]?.verify?.checked],
  () => {
    if (liveTimer) return;
    liveTimer = setTimeout(() => {
      liveTimer = undefined;
      void loadFacets();
      if (rows.value.length <= PAGE) void reload();
    }, 2000);
  },
);
onBeforeUnmount(() => clearTimeout(liveTimer));

// ---- virtual list ----
const scroller = ref<HTMLElement | null>(null);
const virtualizer = useVirtualizer(
  computed(() => ({
    count: rows.value.length,
    getScrollElement: () => scroller.value,
    estimateSize: () => ROW,
    overscan: 20,
  })),
);
const items = computed(() => virtualizer.value.getVirtualItems());
watch(items, (vis) => {
  const last = vis.at(-1);
  if (last && last.index >= rows.value.length - 50) void loadMore();
});
// A new filter starts at the top.
watch(filter, () => scroller.value?.scrollTo({ top: 0 }), { deep: true });

// ---- detail drawer, deep-linkable as ?entry=<idx> ----
const openIdx = computed(() => (route.query.entry != null && route.query.entry !== "" ? Number(route.query.entry) : null));
function open(e: EntryView_Serialize) {
  setQuery({ entry: String(e.idx) });
}
function close() {
  setQuery({ entry: undefined });
}
const focused = ref(0);
function onKey(e: KeyboardEvent) {
  const n = rows.value.length;
  if (!n) return;
  if (e.key === "j" || e.key === "ArrowDown") focused.value = Math.min(n - 1, focused.value + 1);
  else if (e.key === "k" || e.key === "ArrowUp") focused.value = Math.max(0, focused.value - 1);
  else if (e.key === "Enter") return open(rows.value[focused.value]);
  else return;
  e.preventDefault();
  virtualizer.value.scrollToIndex(focused.value);
}

// ---- chips ----
const count = (statuses: EntryStatus[]) => facets.value?.by_status.filter((s) => statuses.includes(s.status)).reduce((n, s) => n + s.files, 0);
const total = computed(() => facets.value?.by_status.reduce((n, s) => n + s.files, 0));
const statusChips = computed(() => [
  { value: "problems" as Chip, label: t("files.chip.problems"), count: facets.value?.problems, icon: "warnCircle" as const },
  { value: "verified" as Chip, label: t("files.chip.verified"), count: count(["verified"]) },
  { value: "sent" as Chip, label: t("files.chip.sent"), count: count(["sent"]) },
  { value: "renamed" as Chip, label: t("files.chip.renamed"), count: facets.value?.renamed, icon: "renamed" as const },
  { value: "skipped" as Chip, label: t("files.chip.skipped"), count: count(["skipped"]) },
  { value: "not_sent" as Chip, label: t("files.chip.notSent"), count: count(["not_sent", "sending"]) },
].filter((c) => c.value === "problems" || (c.count ?? 0) > 0));
const categoryChips = computed(() =>
  (facets.value?.by_category ?? []).map((c) => ({ value: c.category, label: t(`category.${c.category}`), count: c.files, icon: CATEGORY_ICON[c.category], color: categoryColor(c.category) })),
);
const filtered = computed(() => chips.value.length > 0 || categories.value.length > 0 || !!search.value.trim());
function clearFilters() {
  search.value = "";
  setQuery({ status: undefined, cat: undefined });
}
</script>

<template>
  <section class="card files">
    <header class="head">
      <h2>{{ $t("files.title") }} <span v-if="total != null" class="badge badge-muted num">{{ fmt.count(total) }}</span></h2>
      <SearchField v-model="search" :placeholder="$t('files.search')" />
      <button v-if="job && TERMINAL.includes(job.state)" class="btn btn-small" @click="emit('exportReport')"><AppIcon name="export" /> {{ $t("jobs.menu.export") }}</button>
      <label class="sort small">
        <span class="muted">{{ $t("files.sort") }}</span>
        <select v-model="sort">
          <option value="folder_order">{{ $t("files.sortBy.folder_order") }}</option>
          <option value="largest_first">{{ $t("files.sortBy.largest_first") }}</option>
          <option value="problems_first">{{ $t("files.sortBy.problems_first") }}</option>
        </select>
      </label>
    </header>
    <FilterChips v-model="chips" :options="statusChips" :label="$t('files.show')" :all-label="$t('files.chip.all')" :all-count="total" />
    <FilterChips v-if="categoryChips.length > 1" v-model="categories" :options="categoryChips" :label="$t('files.kinds')" />

    <p v-if="error" class="error small" role="alert">{{ error }} <button class="btn btn-small" @click="reload">{{ $t("common.retry") }}</button></p>

    <div class="table-head small muted" aria-hidden="true">
      <span>{{ $t("files.col.name") }}</span><span>{{ $t("files.col.folder") }}</span><span class="n">{{ $t("files.col.size") }}</span><span>{{ $t("files.col.status") }}</span>
    </div>
    <div
      ref="scroller"
      class="scroller"
      role="grid"
      :aria-label="$t('files.title')"
      :aria-rowcount="rows.length"
      tabindex="0"
      @keydown="onKey"
    >
      <div v-if="!rows.length && !loading" class="empty">
        <AppIcon name="search" :size="40" class="muted" />
        <strong>{{ filtered ? $t("files.noMatch") : $t("files.empty") }}</strong>
        <button v-if="filtered" class="btn btn-small" @click="clearFilters">{{ $t("files.clear") }}</button>
      </div>
      <div :style="{ height: `${virtualizer.getTotalSize()}px`, position: 'relative' }">
        <div
          v-for="it in items"
          :key="rows[it.index].idx"
          class="row"
          :class="{ focused: it.index === focused }"
          role="row"
          :aria-rowindex="it.index + 1"
          :style="{ transform: `translateY(${it.start}px)`, height: `${ROW}px`, boxShadow: `inset 3px 0 0 ${categoryColor(rows[it.index].category)}` }"
          @click="(focused = it.index), open(rows[it.index])"
        >
          <span class="cell name" role="gridcell">
            <AppIcon :name="CATEGORY_ICON[rows[it.index].category]" :style="{ color: categoryColor(rows[it.index].category) }" />
            <span class="ellipsis" :title="rows[it.index].name">{{ rows[it.index].name }}</span>
            <AppIcon v-if="rows[it.index].source_name" name="renamed" class="muted" :label="$t('queue.renamed', { name: rows[it.index].source_name })" />
          </span>
          <span class="cell path muted ellipsis" role="gridcell" :title="rows[it.index].dir">{{ rows[it.index].dir || "/" }}</span>
          <span class="cell n num small" role="gridcell">{{ fmt.bytes(rows[it.index].size) }}</span>
          <span class="cell status" role="gridcell"><EntryStatusBadge :status="rows[it.index].status" /></span>
        </div>
      </div>
    </div>
    <p v-if="loading" class="small muted">{{ $t("common.loading") }}</p>

    <EntryDrawer :job-id="jobId" :idx="openIdx" @close="close" />
  </section>
</template>

<style scoped>
.files { display: grid; gap: 0.7rem; container-type: inline-size; }
.head { display: flex; gap: 0.8rem; align-items: center; flex-wrap: wrap; }
.head h2 { font-size: 1rem; margin: 0; display: inline-flex; gap: 0.4rem; align-items: center; }
.sort { display: inline-flex; gap: 0.4rem; align-items: center; margin-left: auto; }
.sort select { width: auto; }
.table-head, .row { display: grid; grid-template-columns: minmax(0, 2.2fr) minmax(0, 2fr) 6.5rem 9.5rem; gap: 0.75rem; align-items: center; padding: 0 0.75rem; }
.table-head { padding-top: 0.25rem; padding-bottom: 0.25rem; border-bottom: 1px solid var(--border); font-weight: 600; }
.scroller { height: min(60vh, 34rem); overflow: auto; position: relative; outline: none; }
.scroller:focus-visible { box-shadow: 0 0 0 2px var(--focus); border-radius: 4px; }
.row { position: absolute; left: 0; right: 0; top: 0; border-bottom: 1px solid var(--rule); cursor: pointer; }
.row:hover, .row.focused { background: var(--surface-2); }
.cell { min-width: 0; }
.name { display: flex; gap: 0.5rem; align-items: center; }
.n { text-align: right; }
.empty { display: grid; justify-items: center; gap: 0.5rem; padding: 2.5rem 1rem; text-align: center; }
/* Columns that can't fit go first: the folder, then each file takes two lines (name and status,
   folder and size) in the same row height. */
@container (max-width: 760px) { .table-head, .row { grid-template-columns: minmax(0, 1fr) 6rem 8.5rem; } .table-head span:nth-child(2), .row .path { display: none; } }
@container (max-width: 520px) {
  .table-head { grid-template-columns: minmax(0, 1fr) auto; }
  .table-head span:nth-child(3) { display: none; }
  .row { grid-template-columns: minmax(0, 1fr) auto; grid-template-areas: "name status" "path size"; gap: 0 0.75rem; align-content: center; }
  .row .name { grid-area: name; }
  .row .path { display: block; grid-area: path; font-size: var(--fs-xs); padding-left: 1.6rem; }
  .row .n { grid-area: size; font-size: var(--fs-xs); color: var(--text-2); }
  .row .status { grid-area: status; justify-self: end; }
}
</style>
