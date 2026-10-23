<script setup lang="ts">
// Two finished transfers' reports side by side: what was added, removed, changed, renamed or newly
// verified from the first to the second, files matched by their source path
// (docs/design/specs/D09-report-export.md, ADR 0025). The service compares; the window shows
// the counts and a page of the changes.
import { computed, onMounted, ref, shallowRef, watch } from "vue";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import type { JobSummary_Serialize, ReportChange_Serialize as ReportChange, ReportChangeKind, ReportDiff_Serialize as ReportDiff, ReportSide_Serialize as ReportSide } from "@/bindings";
import AppIcon from "@/components/AppIcon.vue";
import EntryStatusBadge from "@/components/EntryStatusBadge.vue";
import FilterChips from "@/components/FilterChips.vue";
import { useFmt } from "@/composables/useFmt";
import { api, errorMessage } from "@/lib/api";
import type { IconName } from "@/lib/icons";
import { jobLabel } from "@/lib/jobs";

const props = defineProps<{ a: string; b?: string }>();
const router = useRouter();
const fmt = useFmt();
const { t } = useI18n();

const KINDS: ReportChangeKind[] = ["added", "removed", "changed", "renamed", "newly_verified", "new_problem"];
const LOOK: Record<ReportChangeKind, { icon: IconName; tone: string }> = {
  added: { icon: "plus", tone: "accent" },
  removed: { icon: "missing", tone: "muted" },
  changed: { icon: "different", tone: "warn" },
  renamed: { icon: "renamed", tone: "accent" },
  newly_verified: { icon: "verified", tone: "ok" },
  new_problem: { icon: "warnCircle", tone: "danger" },
};
/** The most the service lists at once. */
const MAX = 5000;
const STEP = 500;

const candidates = shallowRef<JobSummary_Serialize[]>([]);
const diff = shallowRef<ReportDiff | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const kinds = ref<ReportChangeKind[]>([]);
const limit = ref(STEP);

const from = computed({ get: () => props.a, set: (a: string) => go(a, props.b) });
const to = computed({ get: () => props.b ?? "", set: (b: string) => go(props.a, b || undefined) });
function go(a: string, b: string | undefined) {
  void router.replace({ name: "compare", params: { a, b } });
}
function swap() {
  if (props.b) go(props.b, props.a);
}

const byId = computed(() => Object.fromEntries(candidates.value.map((j) => [j.id, j])));
function option(j: JobSummary_Serialize) {
  return `${jobLabel(j)} · ${fmt.date(j.finished_at ?? j.created_at)}`;
}

onMounted(async () => {
  try {
    const page = await api.listJobs({ query: null, groups: ["finished", "cancelled"], profile_id: null, reclaimable: false, sort: "newest" }, 500, 0);
    candidates.value = page.items;
    if (!props.b) suggest();
  } catch (e) {
    error.value = errorMessage(e);
  }
});

/** With no second transfer chosen: the next run of the same folders, else the one before it
 *  (then compared from it to this one, so the changes read forward in time). */
function suggest() {
  const a = byId.value[props.a];
  if (!a) return;
  const same = candidates.value.filter((j) => j.id !== a.id && j.sources.join("\n") === a.sources.join("\n"));
  const later = same.filter((j) => j.created_at > a.created_at).sort((x, y) => x.created_at - y.created_at);
  // `candidates` is newest first: the first earlier one is the run just before.
  const earlier = same.find((j) => j.created_at < a.created_at);
  if (later[0]) go(a.id, later[0].id);
  else if (earlier) go(earlier.id, a.id);
}

// One kind at a time: the service lists the changes of one kind, or all.
watch(kinds, (v) => {
  if (v.length > 1) kinds.value = [v[v.length - 1]];
});
watch([() => props.a, () => props.b, kinds], () => {
  limit.value = STEP;
});
watch(
  [() => props.a, () => props.b, kinds, limit],
  async () => {
    diff.value = null;
    error.value = null;
    if (!props.b) return;
    loading.value = true;
    try {
      diff.value = await api.diffReports(props.a, props.b, kinds.value[0] ?? null, limit.value);
    } catch (e) {
      error.value = errorMessage(e);
    } finally {
      loading.value = false;
    }
  },
  { immediate: true },
);

const chips = computed(() =>
  KINDS.map((k) => ({
    value: k,
    label: t(`report.compare.kind.${k}`),
    icon: LOOK[k].icon,
    count: diff.value?.counts.find((c) => c.kind === k)?.files ?? 0,
  })),
);
/** Back where the user came from (a transfer's menu), else to that transfer. */
function back() {
  if (window.history.state?.back) router.back();
  else void router.push({ name: "job", params: { id: props.a } });
}

/** What to show of one side: only what this change is about. */
function shows(c: ReportChange, side: ReportSide | null) {
  if (!side) return null;
  return {
    path: c.kinds.includes("renamed") ? side.destination_path : null,
    size: c.kinds.includes("changed") ? fmt.bytes(side.size) : null,
    hash: c.kinds.includes("changed") && side.sha256 ? side.sha256.slice(0, 12) : null,
    rules: c.kinds.includes("renamed") && side.renamed_by.length ? side.renamed_by.join(", ") : null,
  };
}
</script>

<template>
  <div>
    <header class="page-head">
      <div>
        <button class="link small" @click="back"><AppIcon name="back" /> {{ $t("common.back") }}</button>
        <h1>{{ $t("report.compare.title") }}</h1>
        <p class="lead-text">{{ $t("report.compare.intro") }}</p>
      </div>
    </header>

    <div class="pick card">
      <label class="field">
        <span>{{ $t("report.compare.from") }}</span>
        <select v-model="from">
          <option v-if="!byId[a]" :value="a">{{ a }}</option>
          <option v-for="j in candidates" :key="j.id" :value="j.id">{{ option(j) }}</option>
        </select>
      </label>
      <button class="btn btn-small swap" :disabled="!b" :title="$t('report.compare.swap')" :aria-label="$t('report.compare.swap')" @click="swap"><AppIcon name="again" /></button>
      <label class="field">
        <span>{{ $t("report.compare.to") }}</span>
        <select v-model="to">
          <option value="">{{ $t("report.compare.choose") }}</option>
          <option v-for="j in candidates" :key="j.id" :value="j.id">{{ option(j) }}</option>
        </select>
      </label>
    </div>

    <p v-if="error" class="error" role="alert">{{ error }}</p>
    <p v-else-if="!b" class="muted">{{ $t("report.compare.pickSecond") }}</p>
    <p v-else-if="loading && !diff" class="muted" aria-live="polite">{{ $t("report.compare.comparing") }}</p>

    <template v-if="diff">
      <p v-if="!diff.same_sources" class="warn-text small"><AppIcon name="warning" /> {{ $t("report.compare.otherSources") }}</p>
      <p v-else-if="!diff.same_profile" class="muted small"><AppIcon name="info" /> {{ $t("report.compare.otherDestination") }}</p>

      <FilterChips v-model="kinds" :options="chips" :label="$t('files.show')" :all-label="$t('files.chip.all')" class="chips" />
      <p class="small muted">{{ $t("report.compare.unchanged", { n: fmt.count(diff.unchanged) }, diff.unchanged) }}</p>

      <div v-if="!diff.changes.length" class="card empty">
        <AppIcon class="empty-icon" name="checkCircle" :size="40" />
        <strong>{{ kinds.length ? $t("report.compare.noneOfKind") : $t("report.compare.same") }}</strong>
      </div>
      <ul v-else class="changes card" :aria-busy="loading">
        <li v-for="c in diff.changes" :key="c.source_path + c.kinds.join()">
          <div class="head">
            <span v-for="k in c.kinds" :key="k" class="badge" :class="`badge-${LOOK[k].tone}`"><AppIcon :name="LOOK[k].icon" :size="12" weight="bold" /> {{ $t(`report.compare.kind.${k}`) }}</span>
            <span class="path">{{ c.source_path }}</span>
          </div>
          <div v-if="c.a && c.b" class="sides small">
            <template v-for="(side, i) in [c.a, c.b]" :key="i">
              <span v-if="i === 1" class="muted" aria-hidden="true">→</span>
              <span class="side">
                <span class="sr-only">{{ i === 0 ? $t("report.compare.before") : $t("report.compare.after") }}</span>
                <EntryStatusBadge :status="side.status" />
                <span v-if="shows(c, side)?.path" class="path">{{ shows(c, side)?.path }}</span>
                <span v-if="shows(c, side)?.size" class="num">{{ shows(c, side)?.size }}</span>
                <span v-if="shows(c, side)?.hash" class="hash" :title="side.sha256 ?? ''">{{ shows(c, side)?.hash }}…</span>
                <span v-if="shows(c, side)?.rules" class="muted">({{ shows(c, side)?.rules }})</span>
              </span>
            </template>
          </div>
        </li>
      </ul>
      <div v-if="diff.more" class="more">
        <span class="small muted">{{ $t("report.compare.showing", { n: fmt.count(diff.changes.length) }) }}</span>
        <button v-if="limit < MAX" class="btn btn-small" :disabled="loading" @click="limit = Math.min(MAX, limit + STEP)">{{ $t("report.compare.showMore") }}</button>
        <span v-else class="small muted">{{ $t("report.compare.useCli") }} <code>filebridge report diff {{ a.slice(0, 8) }} {{ b?.slice(0, 8) }}</code></span>
      </div>
    </template>
  </div>
</template>

<style scoped>
.pick { display: flex; gap: 0.8rem; align-items: end; flex-wrap: wrap; margin-bottom: 1rem; }
.pick .field { flex: 1 1 16rem; }
.swap { margin-bottom: 0.2rem; }
.chips { margin: 0.5rem 0; }
.changes { list-style: none; margin: 0.5rem 0 0; padding: 0; }
.changes li { padding: 0.6rem 0.9rem; border-bottom: 1px solid var(--border); display: grid; gap: 0.3rem; }
.changes li:last-child { border-bottom: 0; }
.head, .sides, .side { display: flex; gap: 0.4rem; align-items: center; flex-wrap: wrap; min-width: 0; }
.head .path { flex: 1 1 12rem; }
.sides { gap: 0.4rem 0.8rem; }
.empty { display: grid; justify-items: center; text-align: center; gap: 0.4rem; padding: 2rem 1rem; margin-top: 0.5rem; }
.empty-icon { color: var(--ok); }
.more { display: flex; gap: 0.6rem; align-items: center; flex-wrap: wrap; margin-top: 0.8rem; }
.page-head .lead-text { margin-bottom: 0; }
</style>
