<script setup lang="ts">
// Scheduled sends (docs/adr/0019): saved sends the engine runs at set times, sending only what is
// new or changed since the last run. A changed file's older version is kept at the destination.
// A watched schedule (docs/adr/0023) runs when its folders change instead: continuous sync,
// copy-only (nothing at the destination is ever deleted).
import { computed, onMounted, ref, shallowRef } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import AppIcon from "@/components/AppIcon.vue";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import JobStateBadge from "@/components/JobStateBadge.vue";
import RelativeTime from "@/components/RelativeTime.vue";
import SegmentedControl from "@/components/SegmentedControl.vue";
import { errorMessage } from "@/lib/api";
import { pickPaths } from "@/lib/platform";
import { useProfiles } from "@/stores/profiles";
import { useSettings } from "@/stores/settings";
import {
  WEEKDAYS,
  dayGroup,
  hm,
  minuteOf,
  sortedDays,
  useSchedules,
  type Recurrence,
  type ScheduleInput,
  type ScheduleView,
} from "@/stores/schedules";
import { DEFAULT_QUIET_SECS, MAX_QUIET_SECS, MIN_QUIET_SECS, type Weekday } from "@/bindings";

const schedules = useSchedules();
const profiles = useProfiles();
const settings = useSettings();
const router = useRouter();
const { t } = useI18n();

onMounted(async () => {
  await Promise.all([schedules.load(), profiles.load(), settings.loadApp()]);
});

/** Whether the send hours (Settings) are closed now, in local time: a watched schedule's changes
 * wait for them to open (docs/adr/0023). */
function sendHoursClosed(): boolean {
  const w = settings.app.send_window;
  if (!w) return false;
  const d = new Date();
  const m = d.getHours() * 60 + d.getMinutes();
  const open = w.start_minute <= w.end_minute ? m >= w.start_minute && m < w.end_minute : m >= w.start_minute || m < w.end_minute;
  return !open;
}

const error = ref<string | null>(null);
/** The schedule whose "Run now" is on its way, and what came of the last one. */
const running = ref<string | null>(null);
const ranNote = ref<Record<string, string>>({});

// ---- the form ------------------------------------------------------------------------------

type Draft = {
  id: string | null;
  name: string;
  sources: string[];
  profileId: string;
  targetDir: string;
  kind: "days" | "hours" | "on_change";
  days: Weekday[];
  time: string;
  every: number;
  /** Seconds without changes before a watched schedule runs. */
  quiet: number;
  verify: boolean;
  enabled: boolean;
};

const draft = ref<Draft | null>(null);
const saving = ref(false);
const formError = ref<string | null>(null);
const editing = computed(() => draft.value?.id != null);

function blank(): Draft {
  return {
    id: null,
    name: "",
    sources: [],
    profileId: profiles.list[0]?.id ?? "",
    targetDir: "",
    kind: "days",
    days: [...WEEKDAYS],
    time: "03:00",
    every: 6,
    quiet: DEFAULT_QUIET_SECS,
    verify: true,
    enabled: true,
  };
}

function edit(s: ScheduleView) {
  const r = s.recurrence;
  draft.value = {
    id: s.id,
    name: s.name,
    sources: [...s.sources],
    profileId: s.profile_id,
    targetDir: s.options.target_dir ?? "",
    kind: r.kind,
    days: r.kind === "days" ? [...r.days] : [...WEEKDAYS],
    time: r.kind === "on_change" ? "03:00" : hm(r.minute),
    every: r.kind === "hours" ? r.every : 6,
    quiet: r.kind === "on_change" ? r.quiet_secs : DEFAULT_QUIET_SECS,
    verify: s.options.verify,
    enabled: s.enabled,
  };
  formError.value = null;
}

function create() {
  draft.value = blank();
  formError.value = null;
}

const kindOptions = computed(() => [
  { value: "days" as const, label: t("schedules.form.onDays") },
  { value: "hours" as const, label: t("schedules.form.everyHours") },
  { value: "on_change" as const, label: t("schedules.form.onChange") },
]);

function toggleDay(d: Weekday) {
  if (!draft.value) return;
  const days = draft.value.days;
  draft.value.days = days.includes(d) ? days.filter((x) => x !== d) : sortedDays([...days, d]);
}

async function addSources(directory: boolean) {
  if (!draft.value) return;
  const picked = await pickPaths({ directory, multiple: true, title: t("schedules.form.pickTitle") });
  const have = new Set(draft.value.sources);
  draft.value.sources = [...draft.value.sources, ...picked.filter((p) => !have.has(p))];
  if (!draft.value.name && draft.value.sources.length) {
    draft.value.name = draft.value.sources[0].split(/[\\/]/).filter(Boolean).pop() ?? "";
  }
}

/** Why the form can't be saved, in words; `null` when it can. */
const formProblem = computed(() => {
  const d = draft.value;
  if (!d) return null;
  if (!d.name.trim()) return t("schedules.form.needName");
  if (!d.sources.length) return t("schedules.form.needSources");
  if (!d.profileId) return t("schedules.form.needDestination");
  if (d.kind === "on_change") {
    return Number.isInteger(d.quiet) && d.quiet >= MIN_QUIET_SECS && d.quiet <= MAX_QUIET_SECS
      ? null
      : t("schedules.form.needQuiet", { min: MIN_QUIET_SECS, max: MAX_QUIET_SECS });
  }
  if (minuteOf(d.time) == null) return t("schedules.form.needTime");
  if (d.kind === "days" && !d.days.length) return t("schedules.form.needDays");
  if (d.kind === "hours" && !(Number.isInteger(d.every) && d.every >= 1 && d.every <= 24)) return t("schedules.form.needHours");
  return null;
});

async function save() {
  const d = draft.value;
  if (!d || formProblem.value) return;
  const minute = minuteOf(d.time) ?? 0;
  const recurrence: Recurrence =
    d.kind === "on_change"
      ? { kind: "on_change", quiet_secs: d.quiet }
      : d.kind === "days"
        ? { kind: "days", days: [...d.days], minute }
        : { kind: "hours", every: d.every, minute };
  const input: ScheduleInput = {
    id: d.id,
    name: d.name.trim(),
    sources: [...d.sources],
    profile_id: d.profileId,
    options: { verify: d.verify, concurrency: 8, target_dir: d.targetDir.trim() || null, keep_older: true },
    recurrence,
    enabled: d.enabled,
  };
  saving.value = true;
  formError.value = null;
  try {
    await schedules.save(input);
    draft.value = null;
  } catch (e) {
    formError.value = errorMessage(e);
  } finally {
    saving.value = false;
  }
}

// ---- the list ------------------------------------------------------------------------------

/** "Every day at 03:00", "Mon, Wed at 22:30", "Every 6 hours from 00:00". */
function describe(r: Recurrence): string {
  if (r.kind === "on_change") return t("schedules.when.onChange", { secs: r.quiet_secs });
  if (r.kind === "hours") return t("schedules.when.hours", { n: r.every, time: hm(r.minute) }, r.every);
  const group = dayGroup(r.days);
  if (group !== "some") return t(`schedules.when.${group}`, { time: hm(r.minute) });
  const days = sortedDays(r.days)
    .map((d) => t(`schedules.days.${d}`))
    .join(", ");
  return t("schedules.when.some", { days, time: hm(r.minute) });
}

function destinationName(s: ScheduleView): string {
  return profiles.byId[s.profile_id]?.name ?? t("schedules.unknownDestination");
}

async function toggle(s: ScheduleView) {
  error.value = null;
  try {
    const r = s.recurrence;
    await schedules.save({
      id: s.id,
      name: s.name,
      sources: [...s.sources],
      profile_id: s.profile_id,
      options: { ...s.options },
      recurrence: r.kind === "days" ? { ...r, days: [...r.days] } : { ...r },
      enabled: !s.enabled,
    });
  } catch (e) {
    error.value = errorMessage(e);
  }
}

async function runNow(s: ScheduleView) {
  running.value = s.id;
  error.value = null;
  try {
    const job = await schedules.runNow(s.id);
    if (job) void router.push({ name: "job", params: { id: job } });
    else ranNote.value = { ...ranNote.value, [s.id]: t("schedules.nothingChanged") };
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    running.value = null;
  }
}

const removing = shallowRef<ScheduleView | null>(null);
const removeBusy = ref(false);
const removeConsequences = computed(() =>
  removing.value ? [t("schedules.remove.stops", { name: removing.value.name }), t("schedules.remove.keepsRuns"), t("schedules.remove.keepsFiles")] : [],
);
async function doRemove() {
  if (!removing.value) return;
  removeBusy.value = true;
  error.value = null;
  try {
    await schedules.remove(removing.value.id);
    removing.value = null;
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    removeBusy.value = false;
  }
}
</script>

<template>
  <div>
    <header class="page-head">
      <h1>{{ $t("schedules.title") }}</h1>
      <button v-if="!draft" class="btn btn-primary" :disabled="!profiles.list.length" @click="create"><AppIcon name="plus" /> {{ $t("schedules.new") }}</button>
    </header>
    <p class="muted intro">{{ $t("schedules.intro") }}</p>
    <p v-if="error" class="error" role="alert">{{ error }}</p>
    <p v-if="schedules.loadError" class="error" role="alert">{{ schedules.loadError }}</p>

    <section v-if="draft" class="card form" :aria-label="editing ? $t('schedules.form.editTitle') : $t('schedules.form.newTitle')">
      <h2>{{ editing ? $t("schedules.form.editTitle") : $t("schedules.form.newTitle") }}</h2>

      <label class="field">
        <span>{{ $t("schedules.form.name") }}</span>
        <input v-model="draft.name" type="text" maxlength="120" />
      </label>

      <div class="field">
        <span>{{ $t("schedules.form.sources") }}</span>
        <ul v-if="draft.sources.length" class="sources">
          <li v-for="p in draft.sources" :key="p">
            <AppIcon name="folder" /> <span class="mono">{{ p }}</span>
            <button v-if="!editing" class="icon-btn" type="button" :aria-label="$t('schedules.form.removeSource')" @click="draft.sources = draft.sources.filter((x) => x !== p)"><AppIcon name="close" /></button>
          </li>
        </ul>
        <div v-if="!editing" class="row">
          <button class="btn btn-small" type="button" @click="addSources(true)"><AppIcon name="addFolder" /> {{ $t("schedules.form.addFolder") }}</button>
          <button class="btn btn-small" type="button" @click="addSources(false)"><AppIcon name="addFile" /> {{ $t("schedules.form.addFiles") }}</button>
        </div>
      </div>

      <label class="field">
        <span>{{ $t("schedules.form.destination") }}</span>
        <select v-model="draft.profileId" :disabled="editing">
          <option v-for="p in profiles.list" :key="p.id" :value="p.id">{{ p.name }}</option>
        </select>
      </label>
      <label class="field">
        <span>{{ $t("schedules.form.targetDir") }}</span>
        <input v-model="draft.targetDir" type="text" :disabled="editing" :placeholder="$t('schedules.form.targetDirPlaceholder')" />
      </label>
      <p v-if="editing" class="small muted">{{ $t("schedules.form.fixed") }}</p>

      <div class="field">
        <span>{{ $t("schedules.form.when") }}</span>
        <SegmentedControl v-model="draft.kind" :options="kindOptions" :label="$t('schedules.form.when')" />
      </div>
      <template v-if="draft.kind === 'on_change'">
        <div class="row">
          <label class="inline">
            {{ $t("schedules.form.quiet") }}
            <input v-model.number="draft.quiet" class="num" type="number" :min="MIN_QUIET_SECS" :max="MAX_QUIET_SECS" step="1" />
            {{ $t("schedules.form.quietUnit") }}
          </label>
        </div>
        <p class="small muted">{{ $t("schedules.form.watchHelp") }}</p>
      </template>
      <div v-if="draft.kind === 'days'" class="row days" role="group" :aria-label="$t('schedules.form.days')">
        <label v-for="d in WEEKDAYS" :key="d" class="day" :class="{ on: draft.days.includes(d) }">
          <input type="checkbox" class="sr-only" :checked="draft.days.includes(d)" @change="toggleDay(d)" />
          {{ $t(`schedules.days.${d}`) }}
        </label>
      </div>
      <div v-if="draft.kind !== 'on_change'" class="row">
        <label v-if="draft.kind === 'hours'" class="inline">
          {{ $t("schedules.form.every") }}
          <input v-model.number="draft.every" class="num" type="number" min="1" max="24" step="1" />
          {{ $t("schedules.form.hoursFrom") }}
        </label>
        <label class="inline">
          {{ draft.kind === "days" ? $t("schedules.form.at") : "" }}
          <input v-model="draft.time" type="time" required />
        </label>
      </div>
      <p v-if="draft.kind !== 'on_change'" class="small muted">{{ $t("schedules.form.localTime") }}</p>

      <label class="check">
        <input v-model="draft.verify" type="checkbox" />
        <span>{{ $t("schedules.form.verify") }}</span>
      </label>
      <label class="check">
        <input v-model="draft.enabled" type="checkbox" />
        <span>{{ draft.kind === "on_change" ? $t("schedules.form.enabledWatch") : $t("schedules.form.enabled") }}</span>
      </label>

      <ul class="small muted promises">
        <li>{{ $t("schedules.form.onlyChanged") }}</li>
        <li>{{ $t("schedules.form.keepsOlder") }}</li>
        <template v-if="draft.kind === 'on_change'">
          <li>{{ $t("schedules.form.neverDeletes") }}</li>
          <li>{{ $t("schedules.form.stillWriting") }}</li>
          <li>{{ $t("schedules.form.watchWindow") }}</li>
        </template>
        <li v-else>{{ $t("schedules.form.waitsWindow") }}</li>
      </ul>

      <div class="row">
        <button class="btn btn-primary" type="button" :disabled="!!formProblem || saving" @click="save">{{ $t("schedules.form.save") }}</button>
        <button class="btn" type="button" :disabled="saving" @click="draft = null">{{ $t("common.cancel") }}</button>
        <span v-if="formError" class="error small" role="alert">{{ formError }}</span>
        <span v-else-if="formProblem" class="small muted">{{ formProblem }}</span>
      </div>
    </section>

    <div v-if="schedules.loaded && !schedules.items.length && !draft" class="card empty">
      <AppIcon name="waiting" :size="28" />
      <p>{{ profiles.list.length ? $t("schedules.empty") : $t("schedules.emptyNoDestination") }}</p>
      <RouterLink v-if="!profiles.list.length" class="btn btn-primary" :to="{ name: 'destinations', query: { new: '1' } }"><AppIcon name="plus" /> {{ $t("dest.new") }}</RouterLink>
    </div>

    <ul class="list">
      <li v-for="s in schedules.items" :key="s.id" class="card schedule" :class="{ off: !s.enabled }">
        <div class="top">
          <div class="title">
            <strong>{{ s.name }}</strong>
            <span class="small muted">{{ describe(s.recurrence) }}</span>
          </div>
          <label class="check switch">
            <input type="checkbox" :checked="s.enabled" @change="toggle(s)" />
            <span class="small">{{ s.enabled ? $t("schedules.on") : $t("schedules.off") }}</span>
          </label>
        </div>
        <p class="small where">
          <span class="mono">{{ s.sources.join(", ") }}</span>
          <AppIcon name="next" />
          <span>{{ destinationName(s) }}<template v-if="s.options.target_dir"> / {{ s.options.target_dir }}</template></span>
        </p>
        <dl class="kv small">
          <template v-if="s.watch">
            <dt>{{ $t("schedules.watch.title") }}</dt>
            <dd>
              <span class="watch" :class="s.watch.mode">
                <AppIcon :name="s.watch.mode === 'events' ? 'review' : s.watch.mode === 'scanning' ? 'again' : 'pause'" />
                {{ s.watch.mode === "idle" && s.enabled ? $t("schedules.watch.starting") : $t(`schedules.watch.${s.watch.mode}`) }}
              </span>
              <span v-if="s.watch.mode === 'scanning' && s.watch.reason" class="muted">— {{ s.watch.reason }}</span>
            </dd>
            <template v-if="s.watch.limit">
              <dt></dt>
              <dd class="muted">
                {{ $t("schedules.watch.limitHelp") }} <code>sudo sysctl fs.inotify.max_user_watches=524288</code>
              </dd>
            </template>
            <dt>{{ $t("schedules.watch.lastChange") }}</dt>
            <dd>
              <RelativeTime v-if="s.watch.last_change_at != null" :at="s.watch.last_change_at" />
              <span v-else class="muted">{{ $t("schedules.watch.noChange") }}</span>
            </dd>
            <template v-if="s.enabled && s.watch.pending_since != null">
              <dt>{{ $t("schedules.watch.pending") }}</dt>
              <dd>
                <span v-if="sendHoursClosed()">{{ $t("schedules.watch.waitHours") }}</span>
                <template v-else-if="s.next_run_at != null">{{ $t("schedules.watch.goes") }} <RelativeTime :at="s.next_run_at" /></template>
              </dd>
            </template>
            <template v-else-if="s.enabled && s.next_run_at != null">
              <dt>{{ $t("schedules.watch.nextCheck") }}</dt>
              <dd><RelativeTime :at="s.next_run_at" /></dd>
            </template>
            <template v-else-if="!s.enabled">
              <dt>{{ $t("schedules.next") }}</dt>
              <dd class="muted">{{ $t("schedules.notOnItsOwn") }}</dd>
            </template>
          </template>
          <template v-else>
            <dt>{{ $t("schedules.next") }}</dt>
            <dd>
              <RelativeTime v-if="s.enabled && s.next_run_at != null" :at="s.next_run_at" />
              <span v-else class="muted">{{ $t("schedules.notOnItsOwn") }}</span>
            </dd>
          </template>
          <dt>{{ $t("schedules.last") }}</dt>
          <dd>
            <template v-if="s.last_run_at != null && s.last_outcome">
              <RelativeTime :at="s.last_run_at" /> ·
              <span :class="{ 'error-text': s.last_outcome === 'failed' }">{{ $t(`schedules.outcome.${s.last_outcome}`) }}</span>
              <span v-if="s.last_error" class="error-text"> — {{ s.last_error }}</span>
            </template>
            <span v-else class="muted">{{ $t("schedules.never") }}</span>
          </dd>
          <template v-if="s.last_job_id">
            <dt>{{ $t("schedules.lastRun") }}</dt>
            <dd>
              <RouterLink :to="{ name: 'job', params: { id: s.last_job_id } }">{{ $t("schedules.openRun") }}</RouterLink>
              <JobStateBadge v-if="s.last_job_state" :state="s.last_job_state" />
            </dd>
          </template>
          <dt>{{ $t("schedules.sent") }}</dt>
          <dd class="num">{{ $t("schedules.files", { n: s.files_sent }, s.files_sent) }}</dd>
        </dl>
        <div class="row">
          <button class="btn btn-small" :disabled="running != null" @click="runNow(s)"><AppIcon name="play" /> {{ running === s.id ? $t("schedules.running") : $t("schedules.runNow") }}</button>
          <button class="btn btn-small" :disabled="!!draft" @click="edit(s)">{{ $t("schedules.edit") }}</button>
          <button class="btn btn-small btn-danger-quiet" @click="removing = s">{{ $t("schedules.remove.action") }}</button>
          <span v-if="ranNote[s.id]" class="small muted" role="status">{{ ranNote[s.id] }}</span>
        </div>
      </li>
    </ul>

    <ConfirmDialog
      :open="!!removing"
      :title="$t('schedules.remove.title')"
      :consequences="removeConsequences"
      :confirm-label="$t('schedules.remove.confirm')"
      danger
      :busy="removeBusy"
      @confirm="doRemove"
      @cancel="removing = null"
    />
  </div>
</template>

<style scoped>
.page-head { display: flex; justify-content: space-between; align-items: center; gap: 1rem; }
.intro { max-width: 720px; margin-top: 0; }
.form { display: grid; gap: 0.75rem; max-width: 640px; margin-bottom: 1rem; }
.form h2 { font-size: 1rem; margin: 0; }
.sources { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.25rem; }
.sources li { display: flex; gap: 0.4rem; align-items: center; word-break: break-all; }
.icon-btn { background: none; border: 0; padding: 0.1rem; color: var(--text-2); cursor: pointer; display: inline-flex; }
.row { display: flex; gap: 0.5rem 0.75rem; align-items: center; flex-wrap: wrap; }
.row input, .row select { width: auto; }
.num { width: 5rem; }
.inline { display: inline-flex; gap: 0.4rem; align-items: center; }
.days { gap: 0.35rem; }
.day { padding: 0.3rem 0.6rem; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); cursor: pointer; color: var(--text-2); }
.day.on { background: var(--accent-soft); color: var(--accent); border-color: var(--accent); font-weight: 600; }
.day:has(input:focus-visible) { outline: 2px solid var(--focus); outline-offset: 1px; }
.check { display: flex; gap: 0.6rem; align-items: center; cursor: pointer; }
.promises { margin: 0; padding-left: 1.2rem; display: grid; gap: 0.2rem; }
.list { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.75rem; max-width: 820px; }
.schedule { display: grid; gap: 0.5rem; }
.schedule.off .title strong { color: var(--text-2); }
.top { display: flex; justify-content: space-between; align-items: flex-start; gap: 1rem; }
.title { display: grid; gap: 0.15rem; }
.where { display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap; margin: 0; word-break: break-all; }
.kv { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 0.3rem 1rem; margin: 0; }
.kv dt { color: var(--text-2); }
.kv dd { margin: 0; display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap; }
.empty { display: grid; justify-items: center; gap: 0.5rem; padding: 2rem; color: var(--text-2); text-align: center; max-width: 640px; }
.error-text { color: var(--danger); }
.watch { display: inline-flex; gap: 0.3rem; align-items: center; font-weight: 600; }
.watch.events { color: var(--ok); }
.watch.scanning { color: var(--warn-strong); }
.watch.idle { color: var(--text-2); font-weight: 400; }
</style>
