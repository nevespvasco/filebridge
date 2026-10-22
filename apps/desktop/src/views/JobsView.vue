<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { onMounted } from "vue";
import type { JobState } from "@/bindings";
import { useRouter } from "vue-router";
import HelpTip from "@/components/HelpTip.vue";
import JobStateBadge from "@/components/JobStateBadge.vue";
import ProgressBar from "@/components/ProgressBar.vue";
import { useFmt } from "@/composables/useFmt";
import { baseName } from "@/lib/format";
import { PAGE_SIZE, RUNNING, useJobs } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";

const jobs = useJobs();
const profiles = useProfiles();
const router = useRouter();
const fmt = useFmt();
/** Speed and ETA mean something only while files are moving. */
const sending = (state: JobState) => RUNNING.includes(state) && state !== "waiting";

onMounted(() => {
  void jobs.loadPage(0);
  void profiles.load();
});

function sourcesLabel(sources: string[]) {
  if (!sources.length) return "—";
  const first = baseName(sources[0]);
  return sources.length > 1 ? `${first} +${sources.length - 1}` : first;
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
    <p v-if="jobs.loadError" class="error">{{ jobs.loadError }}</p>
    <div v-else-if="!jobs.page.length" class="card empty">
      <AppIcon class="empty-icon" name="tray" :size="40" />
      <strong>{{ $t("jobs.empty") }}</strong>
      <p class="small muted">{{ $t("jobs.emptyHint") }}</p>
      <button class="btn btn-primary" @click="router.push({ name: 'new' })"><AppIcon name="plus" /> {{ $t("jobs.emptyCta") }}</button>
    </div>

    <div v-else class="table-wrap card">
      <table class="table">
        <thead>
          <tr>
            <th class="c-what">{{ $t("jobs.col.what") }}</th>
            <th class="c-where">{{ $t("jobs.col.where") }}</th>
            <th class="c-state">{{ $t("jobs.col.state") }}</th>
            <th class="c-progress">{{ $t("jobs.col.progress") }}</th>
            <th class="c-num">{{ $t("jobs.col.speed") }}</th>
            <th class="c-num">{{ $t("jobs.col.eta") }}</th>
            <th class="c-retries"><span class="th-help">{{ $t("jobs.col.retries") }}<HelpTip :text="$t('jobs.retriesHelp')" :label="$t('jobs.col.retries')" /></span></th>
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
            @keydown.enter="router.push({ name: 'job', params: { id } })"
          >
            <template v-if="jobs.byId[id]">
              <td class="ellipsis" :title="jobs.byId[id].sources.join('\n')">
                {{ sourcesLabel(jobs.byId[id].sources) }}
                <span class="small muted block">{{ fmt.date(jobs.byId[id].created_at) }}</span>
              </td>
              <td class="ellipsis">{{ profiles.byId[jobs.byId[id].profile_id]?.name ?? "—" }}</td>
              <td><JobStateBadge :state="jobs.byId[id].state" /></td>
              <td>
                <ProgressBar :tone="jobs.byId[id].state === 'completed' ? 'ok' : jobs.byId[id].state === 'failed' ? 'danger' : 'accent'" :value="fmt.percent(jobs.live[id]?.transfer?.bytes_done ?? jobs.byId[id].counts.bytes_done, jobs.live[id]?.transfer?.bytes_total ?? jobs.byId[id].counts.bytes)" />
                <span class="small muted">
                  {{ fmt.bytes(jobs.live[id]?.transfer?.bytes_done ?? jobs.byId[id].counts.bytes_done) }} / {{ fmt.bytes(jobs.byId[id].counts.bytes) }}
                </span>
              </td>
              <td class="nowrap">{{ sending(jobs.byId[id].state) ? fmt.rate(jobs.live[id]?.transfer?.bytes_per_sec) : "—" }}</td>
              <td class="nowrap">{{ sending(jobs.byId[id].state) ? fmt.duration(jobs.live[id]?.transfer?.eta_secs) : "—" }}</td>
              <td>{{ jobs.byId[id].attempt || "—" }}</td>
            </template>
          </tr>
        </tbody>
      </table>
    </div>

    <div v-if="jobs.offset > 0 || jobs.hasMore" class="pager">
      <button class="btn btn-small" :disabled="jobs.offset === 0" @click="jobs.loadPage(Math.max(0, jobs.offset - PAGE_SIZE))"><AppIcon name="back" /> {{ $t("common.previous") }}</button>
      <button class="btn btn-small" :disabled="!jobs.hasMore" @click="jobs.loadPage(jobs.offset + PAGE_SIZE)">{{ $t("common.next") }} <AppIcon name="next" /></button>
    </div>
  </div>
</template>

<style scoped>
/* Below this width the columns would overlap; the card scrolls sideways instead. */
.table { min-width: 820px; }
.c-what { width: 19%; }
.c-where { width: 15%; }
.c-state { width: 15%; }
.c-progress { width: 19%; }
.c-num { width: 10%; }
.c-retries { width: 12%; }
.block { display: block; }
.page-head .lead-text { margin-bottom: 0; }
.empty { display: grid; justify-items: center; text-align: center; gap: 0.4rem; padding: 2rem 1rem; }
.empty p { margin: 0 0 0.5rem; }
.empty-icon { color: var(--muted); }
.th-help { display: inline-flex; align-items: center; flex-wrap: wrap; }
.pager { display: flex; justify-content: center; gap: 1rem; margin-top: 1rem; }
</style>
