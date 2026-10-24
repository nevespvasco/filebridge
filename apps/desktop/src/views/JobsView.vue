<script setup lang="ts">
// Every transfer: search by name, folder or destination, filter by group and destination, sort,
// and act on each from its menu (docs/design/specs/D06-transfers-list.md).
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import type { JobGroup, JobSort, JobState } from "@/bindings";
import AppIcon from "@/components/AppIcon.vue";
import EmptyState from "@/components/EmptyState.vue";
import LoadError from "@/components/LoadError.vue";
import FilterChips from "@/components/FilterChips.vue";
import HelpTip from "@/components/HelpTip.vue";
import SkeletonRows from "@/components/SkeletonRows.vue";
import JobActions from "@/components/JobActions.vue";
import JobStateBadge from "@/components/JobStateBadge.vue";
import ProgressBar from "@/components/ProgressBar.vue";
import RelativeTime from "@/components/RelativeTime.vue";
import ReportDialog from "@/components/ReportDialog.vue";
import SearchField from "@/components/SearchField.vue";
import { useFmt } from "@/composables/useFmt";
import { defaultLabel, jobLabel } from "@/lib/jobs";
import { PAGE_SIZE, RUNNING, useJobs } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";
import { useConnection } from "@/stores/connection";

const jobs = useJobs();
const conn = useConnection();
const profiles = useProfiles();
const route = useRoute();
const router = useRouter();
const fmt = useFmt();
const { t } = useI18n();
/** Speed and ETA mean something only while files are moving. */
const sending = (state: JobState) => RUNNING.includes(state) && state !== "waiting";
/** A progress bar while it runs or is paused; otherwise the file count. */
const showsBar = (state: JobState) => RUNNING.includes(state) || state === "paused";

// Filters live in the route query (Back restores them); the search text only in memory.
const GROUPS: JobGroup[] = ["active", "needs_you", "not_sent", "finished", "cancelled"];
const list = (v: unknown) => (typeof v === "string" && v ? v.split(",") : []);
const groups = computed<JobGroup[]>({
  get: () => list(route.query.group).filter((g): g is JobGroup => GROUPS.includes(g as JobGroup)),
  set: (v) => setQuery({ group: v.join(",") || undefined }),
});
const profileId = computed<string>({
  get: () => (typeof route.query.profile === "string" ? route.query.profile : ""),
  set: (v) => setQuery({ profile: v || undefined }),
});
const sort = computed<JobSort>({
  get: () => (["oldest", "largest"].includes(route.query.sort as string) ? (route.query.sort as JobSort) : "newest"),
  set: (v) => setQuery({ sort: v === "newest" ? undefined : v }),
});
const search = ref(jobs.filter.query ?? "");
function setQuery(patch: Record<string, string | undefined>) {
  void router.replace({ query: { ...route.query, ...patch } });
}

watch(
  [groups, profileId, sort, search],
  () => {
    jobs.filter = { query: search.value.trim() || null, groups: groups.value, profile_id: profileId.value || null, reclaimable: false, sort: sort.value };
    void jobs.loadPage(0);
  },
  { immediate: true },
);
onMounted(() => void profiles.load());

const groupChips = computed(() =>
  GROUPS.map((g) => ({ value: g, label: t(`jobs.group.${g}`), count: jobs.groupCounts.find((c) => c.group === g)?.total })),
);
/** "All" counts every group with the current search and destination; no count before the
 *  service has answered, rather than a false 0. */
const allCount = computed(() => (jobs.groupCounts.length ? jobs.groupCounts.reduce((n, c) => n + c.total, 0) : undefined));
const reportFor = ref<string | null>(null);
const filtered = computed(() => groups.value.length > 0 || !!profileId.value || !!search.value.trim());
function clearFilters() {
  search.value = "";
  setQuery({ group: undefined, profile: undefined });
}
</script>

<template>
  <div>
    <header class="page-head">
      <div>
        <h1>{{ $t("jobs.title") }}</h1>
        <p class="lead-text">{{ $t("jobs.intro") }}</p>
      </div>
    </header>

    <!-- Nothing to search or filter until the engine has answered. -->
    <template v-if="!(conn.offline && !jobs.page.length && !filtered)">
      <div class="toolbar">
        <SearchField v-model="search" :placeholder="$t('jobs.search')" />
        <label class="picker" :class="{ set: profileId }">
          <span class="muted">{{ $t("jobs.destination") }}</span>
          <select v-model="profileId">
            <option value="">{{ $t("jobs.allDestinations") }}</option>
            <option v-for="p in profiles.list" :key="p.id" :value="p.id">{{ p.name }}</option>
          </select>
          <AppIcon name="caretDown" class="chevron" />
        </label>
        <label class="picker">
          <span class="muted">{{ $t("files.sort") }}</span>
          <select v-model="sort">
            <option value="newest">{{ $t("jobs.sort.newest") }}</option>
            <option value="oldest">{{ $t("jobs.sort.oldest") }}</option>
            <option value="largest">{{ $t("jobs.sort.largest") }}</option>
          </select>
          <AppIcon name="caretDown" class="chevron" />
        </label>
      </div>
      <FilterChips v-model="groups" :options="groupChips" :label="$t('files.show')" :all-label="$t('files.chip.all')" :all-count="allCount" class="chips" />
    </template>

    <!-- With the engine down, a page already read stays on screen, greyed. -->
    <LoadError v-if="jobs.loadError && !(conn.offline && jobs.page.length)" :message="$t('load.transfers')" :error="jobs.loadError" @retry="jobs.loadPage()" />
    <SkeletonRows v-else-if="jobs.loading && !jobs.page.length" :rows="4" class="card" />
    <EmptyState v-else-if="!jobs.page.length && !filtered" icon="tray" :title="$t('jobs.empty')" :hint="$t('jobs.emptyHint')">
      <button class="btn btn-primary" @click="router.push({ name: 'new' })"><AppIcon name="plus" /> {{ $t("jobs.emptyCta") }}</button>
    </EmptyState>
    <EmptyState v-else-if="!jobs.page.length" icon="search" :title="$t('jobs.noMatch')">
      <button class="btn btn-small" @click="clearFilters">{{ $t("files.clear") }}</button>
    </EmptyState>

    <div v-else class="table-wrap card" :class="{ stale: conn.offline }">
      <!-- Explicit roles: a narrow window shows the rows as cards, and WebKit drops a table's
           semantics once it is no longer displayed as one. -->
      <table class="table" role="table">
        <thead role="rowgroup">
          <tr role="row">
            <th class="c-what" scope="col" role="columnheader">{{ $t("jobs.col.transfer") }}</th>
            <th class="c-where" scope="col" role="columnheader">{{ $t("jobs.col.where") }}</th>
            <th class="c-state" scope="col" role="columnheader">{{ $t("jobs.col.state") }}</th>
            <th class="c-progress" scope="col" role="columnheader">
              <span class="th-help">{{ $t("jobs.col.progress") }}<HelpTip :text="$t('jobs.progressHelp')" :label="$t('jobs.col.progress')" /></span>
            </th>
            <th class="c-size n" scope="col" role="columnheader">{{ $t("jobs.col.size") }}</th>
            <th class="c-when" scope="col" role="columnheader">{{ $t("jobs.col.when") }}</th>
            <th class="c-menu" scope="col" role="columnheader"><span class="sr-only">{{ $t("jobs.col.actions") }}</span></th>
          </tr>
        </thead>
        <tbody role="rowgroup">
          <tr
            role="row"
            v-for="id in jobs.page"
            :key="id"
            tabindex="0"
            class="clickable"
            :title="$t('jobs.rowTip')"
            @click="router.push({ name: 'job', params: { id } })"
            @keydown.enter.self="router.push({ name: 'job', params: { id } })"
          >
            <template v-if="jobs.byId[id]">
              <td class="c-what">
                <strong class="ellipsis block" :title="jobLabel(jobs.byId[id])">{{ jobLabel(jobs.byId[id]) }}</strong>
                <span v-if="jobs.byId[id].label" class="small muted ellipsis block" :title="jobs.byId[id].sources.join('\n')">{{ defaultLabel(jobs.byId[id].sources) }}</span>
                <span v-if="jobs.byId[id].again_of" class="small muted block"><AppIcon name="again" /> {{ $t("jobs.again") }}</span>
              </td>
              <td class="c-where ellipsis" :title="profiles.byId[jobs.byId[id].profile_id]?.name">{{ profiles.byId[jobs.byId[id].profile_id]?.name ?? "—" }}</td>
              <td class="c-state"><JobStateBadge :state="jobs.byId[id].state" :job="jobs.byId[id]" /></td>
              <td class="c-progress" :class="{ 'has-bar': showsBar(jobs.byId[id].state) }">
                <template v-if="showsBar(jobs.byId[id].state)">
                  <ProgressBar
                    :tone="jobs.byId[id].state === 'paused' || jobs.byId[id].state === 'retrying' ? 'warn' : 'accent'"
                    :value="fmt.percent(jobs.live[id]?.transfer?.bytes_done ?? jobs.byId[id].counts.bytes_done, jobs.live[id]?.transfer?.bytes_total ?? jobs.byId[id].counts.bytes)"
                  />
                  <span class="small muted num block">
                    {{ Math.round(fmt.percent(jobs.live[id]?.transfer?.bytes_done ?? jobs.byId[id].counts.bytes_done, jobs.byId[id].counts.bytes)) }} %
                    <template v-if="sending(jobs.byId[id].state) && jobs.live[id]?.transfer">
                      · {{ fmt.rate(jobs.live[id]?.transfer?.bytes_per_sec) }} · {{ fmt.duration(jobs.live[id]?.transfer?.eta_secs) }}
                    </template>
                    <template v-if="jobs.byId[id].attempt"> · {{ $t("progress.retries", { n: jobs.byId[id].attempt }) }}</template>
                  </span>
                </template>
                <span v-else-if="jobs.byId[id].state === 'failed' || jobs.byId[id].state === 'cancelled'" class="muted">—</span>
                <span v-else class="small num">{{ $t("jobs.files", { n: fmt.count(jobs.byId[id].counts.files) }, jobs.byId[id].counts.files) }}</span>
              </td>
              <td class="c-size n small num">{{ jobs.byId[id].counts.bytes ? fmt.bytes(jobs.byId[id].counts.bytes) : "—" }}</td>
              <td class="c-when small muted nowrap">
                <RelativeTime :at="jobs.byId[id].finished_at ?? jobs.byId[id].created_at" />
              </td>
              <td class="c-menu" @click.stop @keydown.stop>
                <JobActions :job="jobs.byId[id]" show-open @export-report="reportFor = id" />
              </td>
            </template>
          </tr>
        </tbody>
      </table>
    </div>

    <div v-if="jobs.offset > 0 || jobs.hasMore" class="pager">
      <button class="btn btn-small" :disabled="jobs.offset === 0" @click="jobs.loadPage(Math.max(0, jobs.offset - PAGE_SIZE))"><AppIcon name="back" /> {{ $t("common.previous") }}</button>
      <span class="small muted num">{{ $t("jobs.range", { from: jobs.offset + 1, to: jobs.offset + jobs.page.length, total: jobs.total }) }}</span>
      <button class="btn btn-small" :disabled="!jobs.hasMore" @click="jobs.loadPage(jobs.offset + PAGE_SIZE)">{{ $t("common.next") }} <AppIcon name="next" /></button>
    </div>

    <ReportDialog :job-id="reportFor" @close="reportFor = null" />
  </div>
</template>

<style scoped>
.toolbar { display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; margin-bottom: 0.75rem; }
.toolbar :deep(.search) { max-width: none; }
.toolbar :deep(.search input) { height: 2.25rem; }
/* A label and its list as one control, the same height as the search: WebKit ignores a native
   list's padding, so the list is unstyled and the box around it carries the border. */
.picker { position: relative; display: inline-flex; align-items: center; gap: 0.4rem; height: 2.25rem; padding: 0 0.6rem 0 0.75rem; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); background: var(--surface); font-size: var(--fs-sm); cursor: pointer; min-width: 0; }
.picker:hover { background: var(--surface-2); }
.picker:focus-within { outline: 2px solid var(--focus); outline-offset: 0; }
.picker.set { border-color: var(--accent); }
.picker > span { white-space: nowrap; }
.picker select { appearance: none; -webkit-appearance: none; field-sizing: content; border: 0; background: transparent; padding: 0 1.1rem 0 0; margin-right: -1.1rem; width: auto; max-width: 12rem; font-weight: 600; color: var(--text); cursor: pointer; text-overflow: ellipsis; }
.picker select:focus-visible { outline: none; }
.picker .chevron { color: var(--text-2); pointer-events: none; flex: none; }
.chips { margin-bottom: 1rem; }
.table-wrap { container-type: inline-size; }
.c-what { width: 26%; }
.c-where { width: 15%; }
.c-state { width: 15%; }
.c-progress { width: 20%; }
.c-size { width: 9%; }
.c-when { width: 10%; }
.c-menu { width: 5%; text-align: right; overflow: visible; }
.table td { overflow: visible; }
/* Only the menu needs to spill out of its cell; a long destination name ends in "…". */
.table td.c-where { overflow: hidden; }
.block { display: block; }
.page-head .lead-text { margin-bottom: 0; }
.th-help { display: inline-flex; align-items: center; flex-wrap: wrap; }
.pager { display: flex; justify-content: center; align-items: center; gap: 0.5rem 1rem; margin-top: 1rem; flex-wrap: wrap; }
/* When the search and both lists don't fit on one line, the search gets its own line rather
   than shrinking to a few letters. */
@media (max-width: 759px) {
  .toolbar :deep(.search) { flex-basis: 100%; min-width: 0; }
}
@media (max-width: 559px) {
  /* The two lists share the line under the search, each taking half. */
  .picker { flex: 1 1 0; }
  .picker select { flex: 1; max-width: none; }
}
/* Columns that can't fit go first: Size, then the table becomes a list of cards (D06). */
@container (max-width: 860px) {
  .c-size { display: none; }
  .c-what { width: 30%; }
}
@container (max-width: 700px) {
  /* The headers stay for screen readers; the retries help in them would be a focus stop no one
     sees, and the card already says "N retries". */
  .table thead { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
  .table thead :deep(.help-tip) { display: none; }
  .table, .table tbody { display: block; }
  .table tr { display: flex; flex-wrap: wrap; align-items: center; gap: 0.25rem 0.75rem; padding: 0.7rem 0.6rem 0.7rem 0.9rem; border-bottom: 1px solid var(--border); }
  /* The line break between the name row and the details. */
  .table tr::after { content: ""; order: 4; flex-basis: 100%; height: 0; }
  .table tr:last-child { border-bottom: 0; }
  .table td { display: block; width: auto; padding: 0; border: 0; }
  .table td.c-what { flex: 1 1 0; min-width: 0; order: 1; }
  .table td.c-state { order: 2; }
  .table td.c-menu { order: 3; width: auto; margin: -0.25rem -0.2rem -0.25rem 0; }
  .table td.c-progress.has-bar { order: 5; flex-basis: 100%; }
  .table td.c-where { order: 6; flex: 1 1 0; min-width: 0; font-size: var(--fs-sm); color: var(--text-2); }
  .table td.c-progress:not(.has-bar) { order: 7; color: var(--text-2); }
  .table td.c-size { order: 8; display: block; }
  .table td.c-when { order: 9; }
}
</style>
