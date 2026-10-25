<script setup lang="ts">
// Home: a way in, does anything need me, what is sending, where things went, what happened, and
// what space can be freed (docs/design/specs/D01-home.md), and the pinned folders a new transfer
// starts from (D02). Every section is a bounded query and degrades on its own.
import { computed, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import type { JobSummary_Serialize } from "@/bindings";
import ActivityFeed from "@/components/ActivityFeed.vue";
import AppIcon from "@/components/AppIcon.vue";
import FileDropZone from "@/components/FileDropZone.vue";
import LoadError from "@/components/LoadError.vue";
import JobStateBadge from "@/components/JobStateBadge.vue";
import ProgressBar from "@/components/ProgressBar.vue";
import RelativeTime from "@/components/RelativeTime.vue";
import SkeletonRows from "@/components/SkeletonRows.vue";
import { useFmt } from "@/composables/useFmt";
import { baseName } from "@/lib/format";
import { continueJob, discardWizardIfAsked, jobLabel } from "@/lib/jobs";
import { useConnection } from "@/stores/connection";
import { useHome } from "@/stores/home";
import { useJobs } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";
import { useReclaim } from "@/stores/reclaim";
import { useSourceShortcuts } from "@/stores/sourceShortcuts";
import { useWizard } from "@/stores/wizard";

const home = useHome();
const jobs = useJobs();
const profiles = useProfiles();
const reclaim = useReclaim();
const shortcuts = useSourceShortcuts();
const wizard = useWizard();
const conn = useConnection();
const router = useRouter();
const { t } = useI18n();
const fmt = useFmt();

/** filebridge's three promises, with the same icons wherever they appear. */
const PROMISES = [
  { key: "source", icon: "lock" },
  { key: "names", icon: "renamed" },
  { key: "proof", icon: "shield" },
] as const;

onMounted(() => {
  void home.load();
  void profiles.load();
  void reclaim.loadEligible();
  void shortcuts.loadPinned();
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
function reason(j: JobSummary_Serialize): { text: string; action: string; icon: "hand" | "failCircle" | "retry" | "warning" | "pause" } {
  switch (j.state) {
    case "paused":
      return { text: t("home.reason.paused", { reason: j.error?.split("\n")[0] ?? "" }), action: t("jobDetail.resume"), icon: "pause" };
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
  if (j.state === "awaiting_approval") return continueJob(j, router);
  if (j.state === "paused") return resume(j.id);
  home.markSeen(j.id);
  void router.push({ name: "job", params: { id: j.id } });
}

/** Dropped or picked folders start a new transfer with just them. */
async function onAdd(paths: string[]) {
  if (!(await discardWizardIfAsked())) return;
  wizard.reset();
  wizard.addSources(paths);
  wizard.step = 1;
  void router.push({ name: "new" });
}

const resuming = ref<string | null>(null);
async function resume(id: string) {
  resuming.value = id;
  try {
    await jobs.resume(id);
  } finally {
    resuming.value = null;
  }
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
    </header>

    <section v-if="offline" class="card offline" role="status">
      <AppIcon name="activity" :size="28" class="muted" />
      <div>
        <h2>{{ $t("home.offline.title") }}</h2>
        <p class="muted">{{ $t("home.offline.body") }}</p>
      </div>
    </section>

    <!-- First use: what filebridge promises, and one way in (the grid comes after the first transfer). -->
    <section v-else-if="firstRun" class="card welcome" aria-labelledby="h-welcome">
      <h2 id="h-welcome" class="welcome-title">{{ $t("home.welcome.title") }}</h2>
      <ul class="promises">
        <li v-for="p in PROMISES" :key="p.key">
          <AppIcon :name="p.icon" :size="28" class="promise-icon" />
          <strong>{{ $t(`home.promise.${p.key}.title`) }}</strong>
          <span class="small muted">{{ $t(`home.promise.${p.key}.body`) }}</span>
        </li>
      </ul>
      <div class="welcome-cta">
        <button class="btn btn-primary btn-big" @click="router.push({ name: 'new' })"><AppIcon name="send" /> {{ $t("home.welcome.cta") }}</button>
        <span v-if="!profiles.list.length" class="small muted">{{ $t("home.welcome.noDestination") }}</span>
      </div>
    </section>

    <section v-if="!firstRun && (!offline || home.answered)" class="block drop-block" :aria-label="$t('home.drop')">
      <FileDropZone class="drop" :quiet="home.needs.length > 0" @add="onAdd" />
    </section>

    <div v-if="!firstRun && (!offline || home.answered)" class="grid" :class="{ stale: offline }">
      <div class="main-col">
        <SkeletonRows v-if="!home.loaded" :rows="3" class="card" />
        <section v-if="home.needs.length" class="block" aria-labelledby="h-needs">
          <h2 id="h-needs">{{ $t("home.needsYou") }}</h2>
          <ul class="cards">
            <li v-for="j in home.needs" :key="j.id" class="card need">
              <AppIcon :name="reason(j).icon" :size="20" :class="j.state === 'failed' ? 'error' : 'warn-text'" />
              <div class="text">
                <RouterLink :to="{ name: 'job', params: { id: j.id } }" class="ellipsis row-link"><strong>{{ jobLabel(j) }}</strong> <span class="muted">→ {{ profiles.byId[j.profile_id]?.name ?? "—" }}</span></RouterLink>
                <span class="small muted">{{ reason(j).text }}</span>
              </div>
              <button class="btn btn-small" :disabled="resuming === j.id || offline" :title="offline ? $t('conn.disconnected') : undefined" @click="act(j)">{{ reason(j).action }}</button>
            </li>
          </ul>
        </section>

        <section v-if="home.sending.length" class="block" aria-labelledby="h-sending">
          <h2 id="h-sending">{{ $t("home.nowSending") }}</h2>
          <ul class="cards">
            <li v-for="j in home.sending.slice(0, 3)" :key="j.id" class="card sending">
              <div class="line">
                <RouterLink :to="{ name: 'job', params: { id: j.id } }" class="ellipsis row-link"><strong>{{ jobLabel(j) }}</strong> <span class="muted">→ {{ profiles.byId[j.profile_id]?.name ?? "—" }}</span></RouterLink>
                <JobStateBadge :state="j.state" />
              </div>
              <span v-if="j.state === 'paused' && j.error_code" class="small warn-text">
                {{ j.error_code === "source_unavailable" ? $t("sourceTools.goneLog") : $t("storage.fullLog") }}
              </span>
              <div v-else-if="j.state === 'paused'" class="paused-line">
                <span class="small muted">{{ $t("home.pausedByYou") }}</span>
                <button class="btn btn-small above" :disabled="resuming === j.id || offline" :title="offline ? $t('conn.disconnected') : undefined" @click="resume(j.id)"><AppIcon name="play" /> {{ $t("jobDetail.resume") }}</button>
              </div>
              <template v-else-if="j.state === 'waiting'">
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

        <section v-if="shortcuts.pinned.length" class="block" aria-labelledby="h-pinned">
          <h2 id="h-pinned">{{ $t("sourceTools.homePinned") }}</h2>
          <ul class="pins">
            <li v-for="p in shortcuts.pinned" :key="p.path">
              <button
                class="btn btn-small"
                type="button"
                :disabled="!p.exists"
                :title="p.exists ? p.path : $t('sourceTools.pinnedMissing', { path: p.path })"
                :aria-label="$t('sourceTools.homePinnedSend', { name: baseName(p.path) || p.path })"
                @click="onAdd([p.path])"
              ><AppIcon :name="p.exists ? 'folder' : 'missing'" /> {{ baseName(p.path) || p.path }}</button>
            </li>
          </ul>
        </section>

        <section v-if="home.recent.length" class="block" aria-labelledby="h-recent">
          <div class="block-head">
            <h2 id="h-recent">{{ $t("home.recent") }}</h2>
            <RouterLink :to="{ name: 'jobs' }" class="small">{{ $t("home.allTransfers") }}</RouterLink>
          </div>
          <ul class="recent card">
            <li v-for="j in home.recent.slice(0, 5)" :key="j.id">
              <RouterLink :to="{ name: 'job', params: { id: j.id } }" class="ellipsis r-name row-link"><strong>{{ jobLabel(j) }}</strong></RouterLink>
              <span class="muted ellipsis r-dest">{{ profiles.byId[j.profile_id]?.name ?? "—" }}</span>
              <JobStateBadge :state="j.state" :job="j" class="r-state" />
              <span class="small num n r-size">{{ j.counts.bytes ? fmt.bytes(j.counts.bytes) : "—" }}</span>
              <span class="small muted nowrap r-when"><RelativeTime :at="j.finished_at ?? j.created_at" /></span>
            </li>
          </ul>
        </section>
        <LoadError v-if="home.failed && !offline" compact :message="$t('home.error')" @retry="home.load()" />
      </div>

      <aside class="side-col">
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
.headline { font-size: var(--fs-lg); color: var(--text); }
.grid { display: grid; grid-template-columns: minmax(0, 1.6fr) minmax(0, 1fr); gap: 1.5rem; align-items: start; }
.main-col, .side-col { display: grid; gap: 1.25rem; min-width: 0; }
.block { display: grid; gap: 0.6rem; }
.drop-block { margin-bottom: 1.5rem; }
.block h2 { font-size: var(--fs-xs); text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-2); margin: 0; }
.block-head { display: flex; justify-content: space-between; align-items: baseline; }
.cards { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.5rem; }
.need { display: flex; gap: 0.75rem; align-items: center; }
.need .text { display: grid; flex: 1; min-width: 0; }
.need { position: relative; }
.sending { display: grid; gap: 0.35rem; position: relative; }
.sending:hover, .need:hover { border-color: var(--border-strong); }
/* The name is the row's link; its hit area covers the whole card or row (keyboard: one stop). */
.row-link { color: var(--text); text-decoration: none; }
.row-link::after { content: ""; position: absolute; inset: 0; }
.row-link:focus-visible { outline: none; }
.row-link:focus-visible::after { outline: 2px solid var(--focus); outline-offset: -2px; border-radius: inherit; }
.need .btn, .above { position: relative; z-index: 1; }
.paused-line { display: flex; gap: var(--sp-2); align-items: center; justify-content: space-between; }
.sending .line { display: flex; justify-content: space-between; gap: 0.5rem; align-items: center; }
.recent { list-style: none; margin: 0; padding: 0; container-type: inline-size; }
.recent li { position: relative; display: grid; grid-template-columns: minmax(0, 1.4fr) minmax(0, 1fr) auto 5rem 5.5rem; gap: 0.6rem; align-items: center; padding: 0.55rem 0.9rem; border-bottom: 1px solid var(--rule); }
.recent li:last-child { border-bottom: 0; }
.recent li:hover { background: var(--surface-2); }
.n { text-align: right; }
.reclaim p { margin: 0 0 0.4rem; display: flex; gap: 0.4rem; align-items: center; }
.reclaim ul { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.3rem; }
.reclaim li { display: flex; justify-content: space-between; gap: 0.5rem; }
.welcome { display: grid; gap: var(--sp-5); padding: var(--sp-6) var(--sp-5); justify-items: center; text-align: center; border-radius: var(--radius-lg); }
.welcome-title { margin: 0; font-size: var(--fs-xl); max-width: 30ch; }
.promises { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: var(--sp-4); width: 100%; max-width: 760px; }
.promises li { display: grid; justify-items: center; gap: var(--sp-1); align-content: start; }
.promise-icon { color: var(--ok); margin-bottom: var(--sp-1); }
.welcome-cta { display: grid; gap: var(--sp-2); justify-items: center; }
.offline { display: flex; gap: 1rem; align-items: center; padding: 1.5rem; margin-bottom: 1.5rem; }
.offline h2 { margin: 0 0 0.25rem; font-size: var(--fs-base); }
.offline p { margin: 0; }
@media (max-width: 1099px) { .grid { grid-template-columns: minmax(0, 1fr); } }
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
  .promises { grid-template-columns: minmax(0, 1fr); }
  .welcome { padding: var(--sp-5) var(--sp-4); }
}
.pins { list-style: none; margin: 0; padding: 0; display: flex; flex-wrap: wrap; gap: 0.5rem; }
.pins .btn { max-width: 16rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
