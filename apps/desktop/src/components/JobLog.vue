<script setup lang="ts">
// A job's log as the service recorded it, readable after the window was closed
// (docs/design/specs/D07-job-log-and-activity.md). Newest first, grouped by day; live events only
// trigger a re-read, so the persisted log stays the one source.
import { baseName } from "@/lib/format";
import { computed, onMounted, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import type { LogEntry } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { bcp47 } from "@/i18n";
import { api, errorMessage } from "@/lib/api";
import type { IconName } from "@/lib/icons";
import { useJobs } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";
import AppIcon from "./AppIcon.vue";

const props = defineProps<{ jobId: string }>();
const jobs = useJobs();
const profiles = useProfiles();
const fmt = useFmt();
const { t, te, locale } = useI18n();
const router = useRouter();

/** The folder holding every path an integration flagged for "Repair destination" (Nextcloud's
 *  incompatible encoding), `""` for the destination root; `null` when nothing asks for it. */
function repairFolder(d: Record<string, unknown>): string | null {
  const problems = Array.isArray(d.problems) ? (d.problems as Record<string, unknown>[]) : [];
  const flagged = problems.filter((p) => p.fix === "repair_destination");
  if (!flagged.length) return null;
  const dirs = flagged.map((p) => (typeof p.path === "string" ? p.path.split("/").slice(0, -1) : []));
  const common: string[] = [];
  for (let i = 0; dirs.every((d) => i < d.length && d[i] === dirs[0][i]); i++) common.push(dirs[0][i]);
  return common.join("/");
}

async function repair(folder: string) {
  const job = jobs.byId[props.jobId];
  if (!job) return;
  error.value = null;
  try {
    const id = await api.createRepairJob(job.profile_id, folder || null);
    void router.push({ name: "repair", params: { id } });
  } catch (e) {
    error.value = errorMessage(e);
  }
}

const PAGE = 100;
/** Newest first. */
const entries = shallowRef<LogEntry[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const hasOlder = ref(false);
const onlyProblems = ref(false);
const copied = ref(false);

async function load() {
  loading.value = true;
  try {
    const page = await api.jobLog(props.jobId, null, null, PAGE);
    entries.value = page;
    hasOlder.value = page.length === PAGE;
    error.value = null;
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    loading.value = false;
  }
}

async function loadOlder() {
  const oldest = entries.value.at(-1);
  if (!oldest) return;
  try {
    const page = await api.jobLog(props.jobId, null, oldest.id, PAGE);
    entries.value = [...entries.value, ...page];
    hasOlder.value = page.length === PAGE;
  } catch (e) {
    error.value = errorMessage(e);
  }
}

/** What was recorded since the newest line shown (the answer is oldest first). */
let reading = false;
async function readNew() {
  if (reading) return;
  const newest = entries.value[0];
  if (!newest) return void load();
  reading = true;
  try {
    const page = await api.jobLog(props.jobId, newest.id, null, 500);
    if (page.length) entries.value = [...page.reverse(), ...entries.value];
  } catch {
    // The next event tries again.
  } finally {
    reading = false;
  }
}

onMounted(load);
watch(() => props.jobId, load);

// Every moment the engine writes to the log also reaches the window as an event.
const job = computed(() => jobs.byId[props.jobId]);
const live = computed(() => jobs.live[props.jobId]);
watch(
  () => [job.value?.state, job.value?.updated_at, live.value?.retry?.attempt, live.value?.window?.opens_at],
  () => void readNew(),
);

const shown = computed(() => (onlyProblems.value ? entries.value.filter((e) => e.level !== "info") : entries.value));

/** Lines grouped by local day, newest day first. */
const days = computed(() => {
  const groups: { key: string; label: string; lines: LogEntry[] }[] = [];
  for (const e of shown.value) {
    const d = new Date(e.at);
    const key = d.toDateString();
    let g = groups.at(-1);
    if (!g || g.key !== key) {
      g = { key, label: dayLabel(d), lines: [] };
      groups.push(g);
    }
    g.lines.push(e);
  }
  return groups;
});

function dayLabel(d: Date): string {
  const today = new Date();
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return t("joblog.today");
  if (d.toDateString() === yesterday.toDateString()) return t("joblog.yesterday");
  return d.toLocaleDateString(bcp47(locale.value), { weekday: "long", day: "numeric", month: "long", year: "numeric" });
}

/** `repair`: the folder a "Repair these names" button opens, when the entry asks for one. */
type Line = { icon: IconName; text: string; detail?: string; repair?: string | null };
const n = (e: LogEntry, k: string) => Number((e.data as Record<string, unknown>)?.[k] ?? 0);
const str = (e: LogEntry, k: string) => {
  const v = (e.data as Record<string, unknown>)?.[k];
  return typeof v === "string" ? v : "";
};
const base = (p: string) => baseName(p) || p;

/** The sentence for a recorded event, in the current language. */
function describe(e: LogEntry): Line {
  const d = (e.data ?? {}) as Record<string, unknown>;
  switch (e.kind) {
    case "created": {
      const destination = profiles.byId[str(e, "profile_id")]?.name ?? "—";
      if (typeof d.repair === "string") {
        return { icon: "renamed", text: t("joblog.createdRepair", { folder: d.repair ? `/${d.repair}` : t("repair.root"), destination }) };
      }
      const sources = Array.isArray(d.sources) ? (d.sources as string[]).map(base) : [];
      return { icon: "plus", text: t("joblog.created", { sources: sources.join(", "), destination }) };
    }
    case "options": {
      return { icon: "settings", text: t("joblog.options", { concurrency: n(e, "concurrency"), verify: d.verify ? t("common.yes") : t("common.no") }) };
    }
    case "analysis":
      return {
        icon: "review",
        text: t("joblog.analysis", { files: fmt.count(n(e, "files")), size: fmt.bytes(n(e, "bytes")) }),
        detail: n(e, "scan_errors") ? t("joblog.scanErrors", { n: n(e, "scan_errors") }, n(e, "scan_errors")) : undefined,
      };
    case "decisions": {
      const decision = str(e, "decision");
      return { icon: "check", text: t("joblog.decisions", { count: fmt.count(n(e, "count")), decision: te(`decision.${decision}`) ? t(`decision.${decision}`) : decision }, n(e, "count")) };
    }
    case "state": {
      const to = str(e, "to");
      if (str(e, "error_code") === "destination_full") {
        return { icon: "warning", text: t("storage.fullLog"), detail: str(e, "error") || undefined };
      }
      return { icon: to === "failed" ? "failCircle" : "activity", text: t("joblog.state", { to: te(`state.${to}`) ? t(`state.${to}`) : to }), detail: str(e, "error") || undefined };
    }
    case "retry":
      return { icon: "retry", text: t("joblog.retry", { in: fmt.duration(n(e, "in_secs")), attempt: n(e, "attempt") }), detail: str(e, "error") || undefined };
    case "space_low":
      return { icon: "warning", text: t("storage.runningOutLog", { free: fmt.bytes(n(e, "free_bytes")), left: fmt.bytes(n(e, "left_bytes")) }) };
    case "send_window_wait":
      return { icon: "waiting", text: t("joblog.window", { time: fmt.time(n(e, "opens_at")) }) };
    case "transfer_round":
      return {
        icon: n(e, "failed") ? "warning" : "send",
        text: t("joblog.round", { sent: fmt.count(n(e, "sent")), size: fmt.bytes(n(e, "bytes")) }, n(e, "sent")),
        detail: n(e, "failed") ? `${t("joblog.roundFailed", { n: fmt.count(n(e, "failed")) }, n(e, "failed"))}${str(e, "first_error") ? ` — ${str(e, "first_error")}` : ""}` : undefined,
      };
    case "verify": {
      const off = n(e, "mismatch") + n(e, "missing") + n(e, "unverifiable");
      return off
        ? { icon: "warning", text: t("joblog.verify", { checked: fmt.count(n(e, "checked")), mismatch: n(e, "mismatch"), missing: n(e, "missing"), unverifiable: n(e, "unverifiable") }) }
        : { icon: "verified", text: t("joblog.verifyClean", { checked: fmt.count(n(e, "checked")) }, n(e, "checked")) };
    }
    case "integration": {
      const problems = Array.isArray(d.problems) ? d.problems.length : 0;
      return {
        icon: problems ? "warning" : "info",
        text: t("joblog.integration", { items: fmt.count(n(e, "items")) }),
        detail: problems ? t("joblog.problems", { n: problems }, problems) : undefined,
        repair: repairFolder(d),
      };
    }
    case "integration_checkpoint":
      return { icon: "info", text: t("joblog.integrationCheckpoint") };
    case "finished": {
      const state = str(e, "state");
      const icon: IconName = state === "completed" ? "checkCircle" : state === "failed" ? "failCircle" : state === "cancelled" ? "close" : "warning";
      const key = `joblog.finished.${state}`;
      return { icon, text: te(key) ? t(key, { n: n(e, "warnings") }, n(e, "warnings")) : t("joblog.finished.other"), detail: str(e, "error") || undefined };
    }
    default:
      return { icon: "info", text: t("joblog.unknown", { kind: e.kind }) };
  }
}

async function copy() {
  const text = [...shown.value]
    .reverse()
    .map((e) => {
      const l = describe(e);
      return `${new Date(e.at).toISOString()}  ${l.text}${l.detail ? ` (${l.detail})` : ""}`;
    })
    .join("\n");
  try {
    await navigator.clipboard.writeText(text);
    copied.value = true;
    setTimeout(() => (copied.value = false), 3000);
  } catch (e) {
    error.value = errorMessage(e);
  }
}
</script>

<template>
  <section class="card job-log">
    <header class="head">
      <h2>{{ $t("jobDetail.log") }}</h2>
      <label class="check small">
        <input v-model="onlyProblems" type="checkbox" />
        <span>{{ $t("joblog.onlyProblems") }}</span>
      </label>
      <button class="btn btn-small" :disabled="!shown.length" @click="copy">
        <AppIcon name="copy" /> {{ copied ? $t("joblog.copied") : $t("joblog.copy") }}
      </button>
    </header>
    <p class="small muted">{{ $t("joblog.note") }}</p>

    <p v-if="error" class="error small" role="alert">{{ error }} <button class="btn btn-small" @click="load">{{ $t("common.retry") }}</button></p>
    <p v-if="loading && !entries.length" class="small muted">{{ $t("common.loading") }}</p>
    <p v-else-if="!shown.length && !error" class="small muted">{{ onlyProblems ? $t("joblog.noProblems") : $t("jobDetail.logEmpty") }}</p>

    <div v-for="day in days" :key="day.key" class="day">
      <h3 class="small muted">{{ day.label }}</h3>
      <ol class="lines" reversed>
        <li v-for="e in day.lines" :key="e.id" :class="`lvl-${e.level}`">
          <time class="num small muted" :datetime="new Date(e.at).toISOString()" :title="fmt.date(e.at)">{{ fmt.time(e.at) }}</time>
          <AppIcon class="kind" :name="describe(e).icon" />
          <span class="text">
            {{ describe(e).text }}
            <span v-if="describe(e).detail" class="detail small">{{ describe(e).detail }}</span>
            <button v-if="describe(e).repair != null" class="btn btn-small repair" @click="repair(describe(e).repair!)">
              <AppIcon name="renamed" /> {{ $t("repair.fromProblems") }}
            </button>
          </span>
        </li>
      </ol>
    </div>

    <button v-if="hasOlder" class="btn btn-small" @click="loadOlder">{{ $t("joblog.older") }}</button>
  </section>
</template>

<style scoped>
.job-log { display: grid; gap: 0.5rem; }
.head { display: flex; gap: 1rem; align-items: center; flex-wrap: wrap; }
.head h2 { font-size: 1rem; margin: 0; flex: 1; }
.check { display: inline-flex; gap: 0.4rem; align-items: center; cursor: pointer; }
.day h3 { margin: 0.75rem 0 0.25rem; font-weight: 600; text-transform: capitalize; }
.lines { list-style: none; margin: 0; padding: 0; }
.lines li { display: grid; grid-template-columns: 4.8rem 1.2rem minmax(0, 1fr); gap: 0.5rem; align-items: start; padding: 0.4rem 0; border-bottom: 1px solid var(--rule); }
.lines li:last-child { border-bottom: 0; }
.kind { margin-top: 0.15rem; color: var(--text-2); }
.text { min-width: 0; overflow-wrap: anywhere; }
.detail { display: block; color: var(--text-2); }
.lvl-warn .kind { color: var(--warn-strong); }
.lvl-error .kind, .lvl-error .text { color: var(--danger); }
</style>
