<script setup lang="ts">
// Every transfer: search by name, folder or destination, filter by group and destination, sort,
// and act on each from its menu (docs/design/specs/D06-transfers-list.md).
import { computed, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import type { JobGroup, JobSort, JobState } from "@/bindings";
import AppIcon from "@/components/AppIcon.vue";
import FilterChips from "@/components/FilterChips.vue";
import HelpTip from "@/components/HelpTip.vue";
import JobActions from "@/components/JobActions.vue";
import JobStateBadge from "@/components/JobStateBadge.vue";
import ProgressBar from "@/components/ProgressBar.vue";
import RelativeTime from "@/components/RelativeTime.vue";
import SearchField from "@/components/SearchField.vue";
import { useFmt } from "@/composables/useFmt";
import { defaultLabel, jobLabel } from "@/lib/jobs";
import { PAGE_SIZE, RUNNING, useJobs } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";

const jobs = useJobs();
const profiles = useProfiles();
const route = useRoute();
const router = useRouter();
const fmt = useFmt();
const { t } = useI18n();
/** Speed and ETA mean something only while files are moving. */
const sending = (state: JobState) => RUNNING.includes(state) && state !== "waiting";

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

const groupChips = computed(() => GROUPS.map((g) => ({ value: g, label: t(`jobs.group.${g}`) })));
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
      <button class="btn" :title="$t('jobs.refreshTip')" @click="jobs.loadPage()">{{ $t("common.refresh") }}</button>
    </header>

    <div class="toolbar">
      <SearchField v-model="search" :placeholder="$t('jobs.search')" />
      <label class="small inline">
        <span class="muted">{{ $t("jobs.destination") }}</span>
        <select v-model="profileId">
          <option value="">{{ $t("jobs.allDestinations") }}</option>
          <option v-for="p in profiles.list" :key="p.id" :value="p.id">{{ p.name }}</option>
        </select>
      </label>
      <label class="small inline">
        <span class="muted">{{ $t("files.sort") }}</span>
        <select v-model="sort">
          <option value="newest">{{ $t("jobs.sort.newest") }}</option>
          <option value="oldest">{{ $t("jobs.sort.oldest") }}</option>
          <option value="largest">{{ $t("jobs.sort.largest") }}</option>
        </select>
      </label>
    </div>
    <FilterChips v-model="groups" :options="groupChips" :label="$t('files.show')" :all-label="$t('files.chip.all')" :all-count="filtered && !groups.length ? undefined : jobs.total" class="chips" />

    <p v-if="jobs.loadError" class="error">{{ jobs.loadError }}</p>
    <div v-else-if="!jobs.page.length && !filtered" class="card empty">
      <AppIcon class="empty-icon" name="tray" :size="40" />
      <strong>{{ $t("jobs.empty") }}</strong>
      <p class="small muted">{{ $t("jobs.emptyHint") }}</p>
      <button class="btn btn-primary" @click="router.push({ name: 'new' })"><AppIcon name="plus" /> {{ $t("jobs.emptyCta") }}</button>
    </div>
    <div v-else-if="!jobs.page.length" class="card empty">
      <AppIcon class="empty-icon" name="search" :size="40" />
      <strong>{{ $t("jobs.noMatch") }}</strong>
      <button class="btn btn-small" @click="clearFilters">{{ $t("files.clear") }}</button>
    </div>

    <div v-else class="table-wrap card">
      <table class="table">
        <thead>
          <tr>
            <th class="c-what" scope="col">{{ $t("jobs.col.transfer") }}</th>
            <th class="c-where" scope="col">{{ $t("jobs.col.where") }}</th>
            <th class="c-state" scope="col">{{ $t("jobs.col.state") }}</th>
            <th class="c-progress" scope="col">
              <span class="th-help">{{ $t("jobs.col.progress") }}<HelpTip :text="$t('jobs.retriesHelp')" :label="$t('jobs.col.retries')" /></span>
            </th>
            <th class="c-size n" scope="col">{{ $t("jobs.col.size") }}</th>
            <th class="c-when" scope="col">{{ $t("jobs.col.when") }}</th>
            <th class="c-menu" scope="col"><span class="sr-only">{{ $t("jobs.col.actions") }}</span></th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="id in jobs.page"
            :key="id"
            tabindex="0"
            class="clickable"
            :title="$t('jobs.rowTip')"
            @click="router.push({ name: 'job', params: { id } })"
            @keydown.enter.self="router.push({ name: 'job', params: { id } })"
          >
            <template v-if="jobs.byId[id]">
              <td>
                <strong class="ellipsis block" :title="jobLabel(jobs.byId[id])">{{ jobLabel(jobs.byId[id]) }}</strong>
                <span v-if="jobs.byId[id].label" class="small muted ellipsis block" :title="jobs.byId[id].sources.join('\n')">{{ defaultLabel(jobs.byId[id].sources) }}</span>
                <span v-if="jobs.byId[id].again_of" class="small muted block"><AppIcon name="again" /> {{ $t("jobs.again") }}</span>
              </td>
              <td class="ellipsis">{{ profiles.byId[jobs.byId[id].profile_id]?.name ?? "—" }}</td>
              <td><JobStateBadge :state="jobs.byId[id].state" /></td>
              <td>
                <template v-if="RUNNING.includes(jobs.byId[id].state) || jobs.byId[id].state === 'paused'">
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
              <td class="n small num">{{ jobs.byId[id].counts.bytes ? fmt.bytes(jobs.byId[id].counts.bytes) : "—" }}</td>
              <td class="small muted nowrap">
                <RelativeTime :at="jobs.byId[id].finished_at ?? jobs.byId[id].created_at" />
              </td>
              <td class="c-menu" @click.stop @keydown.stop>
                <JobActions :job="jobs.byId[id]" show-open />
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

  </div>
</template>

<style scoped>
.toolbar { display: flex; gap: 0.8rem; align-items: center; flex-wrap: wrap; margin-bottom: 0.6rem; }
.inline { display: inline-flex; gap: 0.4rem; align-items: center; }
.inline select { width: auto; max-width: 14rem; }
.chips { margin-bottom: 1rem; }
.table { min-width: 680px; }
.c-what { width: 26%; }
.c-where { width: 15%; }
.c-state { width: 15%; }
.c-progress { width: 20%; }
.c-size { width: 9%; }
.c-when { width: 10%; }
.c-menu { width: 5%; text-align: right; overflow: visible; }
.table td { overflow: visible; }
.block { display: block; }
.page-head .lead-text { margin-bottom: 0; }
.empty { display: grid; justify-items: center; text-align: center; gap: 0.4rem; padding: 2rem 1rem; }
.empty p { margin: 0 0 0.5rem; }
.empty-icon { color: var(--muted); }
.th-help { display: inline-flex; align-items: center; flex-wrap: wrap; }
.pager { display: flex; justify-content: center; align-items: center; gap: 1rem; margin-top: 1rem; }
</style>
