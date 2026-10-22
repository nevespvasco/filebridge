<script setup lang="ts">
// Home: does anything need me, what is sending, where things went, what happened, and what space
// can be freed (docs/design/specs/D01-home.md). Every section is a bounded query and degrades on
// its own.
import { computed, onMounted, watch } from "vue";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import type { JobSummary_Serialize } from "@/bindings";
import ActivityFeed from "@/components/ActivityFeed.vue";
import AppIcon from "@/components/AppIcon.vue";
import DestinationSpace from "@/components/DestinationSpace.vue";
import FileDropZone from "@/components/FileDropZone.vue";
import JobStateBadge from "@/components/JobStateBadge.vue";
import ProgressBar from "@/components/ProgressBar.vue";
import RelativeTime from "@/components/RelativeTime.vue";
import { useFmt } from "@/composables/useFmt";
import { jobLabel } from "@/lib/jobs";
import { useConnection } from "@/stores/connection";
import { useHome } from "@/stores/home";
import { useJobs } from "@/stores/jobs";
import { profileKind, useProfiles } from "@/stores/profiles";
import { useReclaim } from "@/stores/reclaim";
import { useWizard } from "@/stores/wizard";

const home = useHome();
const jobs = useJobs();
const profiles = useProfiles();
const reclaim = useReclaim();
const wizard = useWizard();
const conn = useConnection();
const router = useRouter();
const { t } = useI18n();
const fmt = useFmt();

onMounted(() => {
  void home.load();
  void profiles.load().then(() => profiles.loadAllStorage());
  void reclaim.loadEligible();
});
// A job changing state can move it between sections.
watch(
  () => Object.values(jobs.byId).map((j) => `${j.id}:${j.state}`).join(","),
  () => home.reloadSoon(),
);

/** The worst current condition, in one sentence. */
const headline = computed(() => {
  if (conn.connected === false) return t("home.headline.offline");
  if (!home.loaded) return t("common.loading");
  if (home.needs.length) return t("home.headline.needsYou", { n: home.needs.length }, home.needs.length);
  if (home.sending.length) return t("home.headline.sending", { n: home.sending.length }, home.sending.length);
  if (!home.totalJobs) return t("home.headline.none");
  return t("home.headline.allGood");
});
/** Only when the service answered that there are none: a failed load is not "new user". */
const firstRun = computed(() => conn.connected === true && home.loaded && !home.failed && !home.totalJobs);
const offline = computed(() => conn.connected === false);

/** Why a job needs the user, and the one action for it. */
function reason(j: JobSummary_Serialize): { text: string; action: string; icon: "hand" | "failCircle" | "retry" | "warning" } {
  switch (j.state) {
    case "awaiting_approval":
      return { text: t("home.reason.decide"), action: t("home.action.decide"), icon: "hand" };
    case "failed":
      return { text: j.error?.split(/[.\n]/)[0] || t("home.reason.failed"), action: t("home.action.open"), icon: "failCircle" };
    case "retrying":
      return { text: t("home.reason.retrying", { n: j.attempt }), action: t("home.action.open"), icon: "retry" };
    default:
      return { text: t("home.reason.warnings"), action: t("home.action.review"), icon: "warning" };
  }
}
async function act(j: JobSummary_Serialize) {
  if (j.state === "awaiting_approval") {
    await wizard.openJob(j);
    return void router.push({ name: "new" });
  }
  home.markSeen(j.id);
  void router.push({ name: "job", params: { id: j.id } });
}

/** Dropped or picked folders start a new transfer with them. */
function onAdd(paths: string[]) {
  if (wizard.jobId) wizard.reset();
  wizard.addSources(paths);
  wizard.step = 1;
  void router.push({ name: "new" });
}

const live = (id: string) => jobs.live[id]?.transfer;
const inFlightNames = (id: string) => (live(id)?.in_flight ?? []).map((f) => f.name);
</script>

<template>
  <div class="home">
    <header class="page-head">
      <div>
        <h1>{{ $t("home.title") }}</h1>
        <p class="lead-text headline" aria-live="polite">{{ headline }}</p>
      </div>
      <button class="btn btn-primary" @click="router.push({ name: 'new' })"><AppIcon name="send" /> {{ $t("nav.new") }}</button>
    </header>

    <section v-if="offline" class="card offline" role="status">
      <AppIcon name="activity" :size="28" class="muted" />
      <div>
        <h2>{{ $t("home.offline.title") }}</h2>
        <p class="muted">{{ $t("home.offline.body") }}</p>
      </div>
    </section>

    <section v-else-if="firstRun" class="card onboarding" :aria-label="$t('home.start.title')">
      <h2>{{ $t("home.start.title") }}</h2>
      <ol>
        <li :class="{ done: profiles.list.length > 0 }">
          <AppIcon :name="profiles.list.length ? 'checkCircle' : 'destinations'" />
          <span>{{ $t("home.start.destination") }}</span>
          <button v-if="!profiles.list.length" class="btn btn-small" @click="router.push({ name: 'destinations' })">{{ $t("dest.new") }}</button>
        </li>
        <li><AppIcon name="addFolder" /> <span>{{ $t("home.start.folders") }}</span></li>
        <li><AppIcon name="send" /> <span>{{ $t("home.start.send") }}</span></li>
      </ol>
    </section>

    <div v-if="!offline" class="grid">
      <div class="main-col">
        <section v-if="home.needs.length" class="block" aria-labelledby="h-needs">
          <h2 id="h-needs">{{ $t("home.needsYou") }}</h2>
          <ul class="cards">
            <li v-for="j in home.needs" :key="j.id" class="card need">
              <AppIcon :name="reason(j).icon" :size="20" :class="j.state === 'failed' ? 'error' : 'warn-text'" />
              <div class="text">
                <strong class="ellipsis">{{ jobLabel(j) }} <span class="muted">→ {{ profiles.byId[j.profile_id]?.name ?? "—" }}</span></strong>
                <span class="small muted">{{ reason(j).text }}</span>
              </div>
              <button class="btn btn-small btn-primary" @click="act(j)">{{ reason(j).action }}</button>
            </li>
          </ul>
        </section>

        <section v-if="home.sending.length" class="block" aria-labelledby="h-sending">
          <h2 id="h-sending">{{ $t("home.nowSending") }}</h2>
          <ul class="cards">
            <li v-for="j in home.sending.slice(0, 3)" :key="j.id" class="card sending" @click="router.push({ name: 'job', params: { id: j.id } })">
              <div class="line">
                <strong class="ellipsis">{{ jobLabel(j) }} <span class="muted">→ {{ profiles.byId[j.profile_id]?.name ?? "—" }}</span></strong>
                <JobStateBadge :state="j.state" />
              </div>
              <template v-if="j.state === 'waiting'">
                <span class="small warn-text">{{ jobs.live[j.id]?.window ? $t("progress.window", { time: fmt.time(jobs.live[j.id]!.window!.opens_at) }) : $t("state.waiting") }}</span>
              </template>
              <template v-else>
                <ProgressBar :value="fmt.percent(live(j.id)?.bytes_done ?? j.counts.bytes_done, live(j.id)?.bytes_total ?? j.counts.bytes)" :tone="j.state === 'retrying' ? 'warn' : 'accent'" />
                <span class="small muted num">
                  {{ fmt.bytes(live(j.id)?.bytes_done ?? j.counts.bytes_done) }} / {{ fmt.bytes(live(j.id)?.bytes_total ?? j.counts.bytes) }}
                  <template v-if="live(j.id)"> · {{ fmt.rate(live(j.id)!.bytes_per_sec) }} · {{ $t("progress.eta", { eta: fmt.duration(live(j.id)!.eta_secs) }) }}</template>
                </span>
                <span v-if="inFlightNames(j.id).length" class="small muted ellipsis">
                  {{ inFlightNames(j.id).slice(0, 2).join(", ") }}<template v-if="inFlightNames(j.id).length > 2"> {{ $t("home.more", { n: inFlightNames(j.id).length - 2 }) }}</template>
                </span>
              </template>
            </li>
          </ul>
          <p v-if="home.sending.length > 3" class="small muted">{{ $t("home.more", { n: home.sending.length - 3 }) }}</p>
        </section>

        <section class="block" :aria-label="$t('home.drop')">
          <FileDropZone class="drop" @add="onAdd" />
        </section>

        <section v-if="home.recent.length" class="block" aria-labelledby="h-recent">
          <div class="block-head">
            <h2 id="h-recent">{{ $t("home.recent") }}</h2>
            <RouterLink :to="{ name: 'jobs' }" class="small">{{ $t("home.allTransfers") }}</RouterLink>
          </div>
          <ul class="recent card">
            <li v-for="j in home.recent.slice(0, 5)" :key="j.id" @click="router.push({ name: 'job', params: { id: j.id } })">
              <strong class="ellipsis r-name">{{ jobLabel(j) }}</strong>
              <span class="muted ellipsis r-dest">{{ profiles.byId[j.profile_id]?.name ?? "—" }}</span>
              <JobStateBadge :state="j.state" class="r-state" />
              <span class="small num n r-size">{{ j.counts.bytes ? fmt.bytes(j.counts.bytes) : "—" }}</span>
              <span class="small muted nowrap r-when"><RelativeTime :at="j.finished_at ?? j.created_at" /></span>
            </li>
          </ul>
        </section>
        <p v-if="home.failed" class="error small">{{ $t("home.error") }} <button class="btn btn-small" @click="home.load()">{{ $t("common.retry") }}</button></p>
      </div>

      <aside class="side-col">
        <section class="block" aria-labelledby="h-dest">
          <h2 id="h-dest">{{ $t("nav.destinations") }}</h2>
          <ul class="tiles">
            <li v-for="p in profiles.list" :key="p.id" class="card tile">
              <div class="tile-head">
                <AppIcon :name="profileKind(p)" :size="20" class="muted" />
                <strong class="ellipsis">{{ p.name }}</strong>
              </div>
              <DestinationSpace :profile-id="p.id" compact />
            </li>
          </ul>
          <button class="btn btn-small" @click="router.push({ name: 'destinations' })"><AppIcon name="plus" /> {{ $t("dest.new") }}</button>
        </section>

        <section v-if="reclaim.eligible.length" class="block" aria-labelledby="h-reclaim">
          <h2 id="h-reclaim">{{ $t("home.reclaimable") }}</h2>
          <div class="card reclaim">
            <p><AppIcon name="broom" class="ok-text" /> {{ $t("home.reclaimTotal", { size: fmt.bytes(reclaim.totalBytes) }) }}</p>
            <ul>
              <li v-for="s in reclaim.eligible" :key="s.job_id">
                <RouterLink :to="{ name: 'job', params: { id: s.job_id } }">{{ jobs.byId[s.job_id] ? jobLabel(jobs.byId[s.job_id]) : s.job_id.slice(0, 8) }}</RouterLink>
                <span class="small muted num">{{ fmt.bytes(s.bytes) }}</span>
              </li>
            </ul>
          </div>
        </section>

        <section class="block" aria-labelledby="h-activity">
          <h2 id="h-activity">{{ $t("home.activity") }}</h2>
          <div class="card"><ActivityFeed :limit="8" /></div>
        </section>
      </aside>
    </div>
  </div>
</template>

<style scoped>
.headline { font-size: 1.05rem; color: var(--text); }
.grid { display: grid; grid-template-columns: minmax(0, 1.6fr) minmax(0, 1fr); gap: 1.5rem; align-items: start; }
.main-col, .side-col { display: grid; gap: 1.25rem; min-width: 0; }
.block { display: grid; gap: 0.6rem; }
.block h2 { font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-2); margin: 0; }
.block-head { display: flex; justify-content: space-between; align-items: baseline; }
.cards { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.5rem; }
.need { display: flex; gap: 0.75rem; align-items: center; }
.need .text { display: grid; flex: 1; min-width: 0; }
.sending { display: grid; gap: 0.35rem; cursor: pointer; }
.sending:hover { border-color: var(--border-strong); }
.sending .line { display: flex; justify-content: space-between; gap: 0.5rem; align-items: center; }
.recent { list-style: none; margin: 0; padding: 0; container-type: inline-size; }
.recent li { display: grid; grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr) auto 5rem 5.5rem; gap: 0.6rem; align-items: center; padding: 0.55rem 0.9rem; border-bottom: 1px solid var(--rule); cursor: pointer; }
.recent li:last-child { border-bottom: 0; }
.recent li:hover { background: var(--surface-2); }
.n { text-align: right; }
.tiles { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.5rem; }
.tile { display: grid; gap: 0.45rem; }
.tile-head { display: flex; gap: 0.5rem; align-items: center; min-width: 0; }
.reclaim p { margin: 0 0 0.4rem; display: flex; gap: 0.4rem; align-items: center; }
.reclaim ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.3rem; }
.reclaim li { display: flex; justify-content: space-between; gap: 0.5rem; }
.onboarding { margin-bottom: 1.25rem; }
.offline { display: flex; gap: 1rem; align-items: center; padding: 1.5rem; }
.offline h2 { margin: 0 0 0.25rem; font-size: 1rem; }
.offline p { margin: 0; }
.onboarding h2 { margin: 0 0 0.6rem; font-size: 1rem; }
.onboarding ol { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.5rem; }
.onboarding li { display: flex; gap: 0.6rem; align-items: center; }
.onboarding li.done { color: var(--ok); }
@media (max-width: 1080px) { .grid { grid-template-columns: minmax(0, 1fr); } }
/* A narrow list: the name and its state on top, where, size and when under them. */
@container (max-width: 520px) {
  .recent li { grid-template-columns: minmax(0, 1fr) auto auto; grid-template-areas: "name name state" "dest size when"; gap: 0.15rem 0.75rem; }
  .r-name { grid-area: name; }
  .r-dest { grid-area: dest; font-size: var(--fs-sm); }
  .r-state { grid-area: state; justify-self: end; }
  .r-size { grid-area: size; }
  .r-when { grid-area: when; }
}
@media (max-width: 559px) {
  .need { flex-wrap: wrap; }
  .need .text { flex-basis: calc(100% - 2.5rem); }
  .need .btn { margin-left: auto; }
  .offline { flex-direction: column; align-items: flex-start; padding: 1rem; }
}
</style>
