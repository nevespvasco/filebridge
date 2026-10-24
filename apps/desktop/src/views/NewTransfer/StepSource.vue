<script setup lang="ts">
// Step 1, what to send (docs/design/specs/D02). Each chosen source is checked as it is added
// (what it is, how big, what is wrong with it); Next waits until nothing needs fixing.
import { computed, nextTick, onActivated, onBeforeUnmount, onDeactivated, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { RecentSource } from "@/bindings";
import AppIcon from "@/components/AppIcon.vue";
import FileDropZone from "@/components/FileDropZone.vue";
import RecentSources from "@/components/RecentSources.vue";
import SourceList from "@/components/SourceList.vue";
import TourButton from "@/components/TourButton.vue";
import { useFmt } from "@/composables/useFmt";
import { api } from "@/lib/api";
import { baseName } from "@/lib/format";
import { useTour } from "@/stores/tour";
import { useWizard } from "@/stores/wizard";

const wizard = useWizard();
const tour = useTour();
const fmt = useFmt();
const { t } = useI18n();

const has = computed(() => wizard.sources.length > 0);

// ---- "Recently sent from": re-read when the window comes back, since a disk may have been
// plugged in or out meanwhile; the chosen ones are checked again for the same reason ----
const recent = ref<RecentSource[]>([]);
async function loadRecent() {
  try {
    recent.value = await api.recentSources(12);
  } catch {
    recent.value = [];
  }
}
const recentShown = computed(() =>
  recent.value.filter((r) => !wizard.sources.includes(r.path)).slice(0, has.value ? 3 : 6),
);
function onFocus() {
  void loadRecent();
  if (has.value) void wizard.inspect();
}

/** The guide starts by itself once, after the page has what it points at. */
async function arrive() {
  await loadRecent();
  await nextTick();
  tour.maybeStart("source");
}
onMounted(arrive);
onActivated(arrive);
window.addEventListener("focus", onFocus);
onDeactivated(() => tour.stop("source"));
onBeforeUnmount(() => {
  window.removeEventListener("focus", onFocus);
  tour.stop("source");
  clearTimeout(undoTimer);
});

// ---- removing, with undo ----
type Removed = { path: string; at: number };
const undo = ref<{ items: Removed[]; label: string } | null>(null);
let undoTimer: ReturnType<typeof setTimeout> | undefined;
function offerUndo(items: Removed[]) {
  if (!items.length) return;
  clearTimeout(undoTimer);
  const label = items.length === 1 ? baseName(items[0].path) : t("source.nItems", { n: items.length }, items.length);
  undo.value = { items, label };
  undoTimer = setTimeout(() => (undo.value = null), 8000);
}
function remove(path: string) {
  const at = wizard.removeSource(path);
  if (at >= 0) offerUndo([{ path, at }]);
}
function removeMany(paths: string[]) {
  // Where each one was: undo puts them back in order.
  const all = wizard.sources.map((path, at) => ({ path, at }));
  const gone = all.filter((r) => paths.includes(r.path));
  wizard.sources = wizard.sources.filter((p) => !paths.includes(p));
  offerUndo(gone);
}
function clearAll() {
  removeMany([...wizard.sources]);
}
function restore() {
  if (!undo.value) return;
  wizard.restoreSources(undo.value.items);
  undo.value = null;
  clearTimeout(undoTimer);
}

/** Already inside another chosen source: "Remove the N duplicates". */
const duplicates = computed(() =>
  wizard.sources.filter((p) => wizard.coveredBy[p] && wizard.sources.includes(wizard.coveredBy[p])),
);

// ---- the summary in the footer ----
const summary = computed(() => {
  const known = wizard.sources.map((p) => wizard.sourceInfo[p]).filter((i) => i && !i.problem);
  const folders = known.filter((i) => i.is_dir).length;
  const files = known.length - folders;
  const bytes = known.reduce((n, i) => n + (i.size ?? 0), 0);
  const parts = [t("source.nItems", { n: wizard.sources.length }, wizard.sources.length)];
  if (known.length) {
    const kinds = [
      folders ? t("source.nFolders", { n: folders }, folders) : null,
      files ? t("source.nFiles", { n: files }, files) : null,
    ].filter(Boolean);
    parts.push(kinds.join(", "));
  }
  if (files) parts.push(t("source.summarySize", { size: fmt.bytes(bytes) }));
  // A narrow footer has room for one short line: "1 file · 316 KB" (the count goes once kinds are known).
  const short = known.length ? parts.slice(1) : parts;
  if (files) short[short.length - 1] = fmt.bytes(bytes);
  return { long: parts.join(" · "), short: short.join(" · ") };
});
const blocked = computed(() => wizard.sourceBlockers.length);
</script>

<template>
  <div class="step">
    <div class="title-row">
      <h2>{{ $t("source.title") }}</h2>
      <TourButton tour="source" />
    </div>
    <p class="lead-text">{{ $t("source.lead") }}</p>

    <FileDropZone data-tour="source-add" :compact="has" @add="wizard.addSources" />

    <section v-if="has" class="card chosen" data-tour="source-chosen" :aria-label="$t('source.chosen', { n: wizard.sources.length }, wizard.sources.length)">
      <div class="chosen-head">
        <strong>{{ $t("source.chosen", { n: wizard.sources.length }, wizard.sources.length) }}</strong>
        <span v-if="wizard.inspecting" class="small muted">{{ $t("source.checking") }}</span>
        <span class="grow" />
        <button v-if="duplicates.length" class="btn btn-small" type="button" @click="removeMany(duplicates)">
          {{ $t("source.removeDuplicates", { n: duplicates.length }, duplicates.length) }}
        </button>
        <button class="btn btn-small" type="button" @click="clearAll">{{ $t("source.clearAll") }}</button>
      </div>
      <p v-if="wizard.inspectError" class="small muted">{{ $t("source.checkFailed") }}</p>
      <SourceList
        :sources="wizard.sources"
        :info="wizard.sourceInfo"
        :covered-by="wizard.coveredBy"
        @remove="remove"
        @replace="wizard.replaceSource"
      />
    </section>

    <RecentSources v-if="recentShown.length" class="recent" data-tour="source-recent" :items="recentShown" @add="(p) => wizard.addSources([p])" />

    <div class="footer sticky" data-tour="source-next">
      <span class="summary small" :class="{ 'has-hint': blocked, 'has-undo': undo }" :title="has ? summary.long : undefined" aria-live="polite">
        <template v-if="has"><span class="long">{{ summary.long }}</span><span class="short" aria-hidden="true">{{ summary.short }}</span></template>
      </span>
      <span v-if="undo" class="undo small" role="status">
        {{ $t("source.removed", { what: undo.label }) }}
        <button class="btn btn-small" type="button" @click="restore"><AppIcon name="undo" /> {{ $t("source.undo") }}</button>
      </span>
      <span class="grow" />
      <span v-if="blocked" class="why-disabled warn-text" :class="{ 'has-undo': undo }">{{ $t("source.blocked", { n: blocked }, blocked) }}</span>
      <button class="btn btn-primary" :disabled="!wizard.canLeaveSource" @click="wizard.step = 2">{{ $t("common.next") }} <AppIcon name="next" /></button>
    </div>
  </div>
</template>

<style scoped>
.step { display: grid; gap: 1rem; }
.title-row { display: flex; gap: 0.6rem; align-items: center; }
.title-row h2 { margin: 0; }
.lead-text { margin: -0.4rem 0 0; }
.chosen { padding: 0.75rem 1rem; }
.chosen-head { display: flex; gap: 0.6rem; align-items: center; flex-wrap: wrap; padding-bottom: 0.4rem; border-bottom: 1px solid var(--border); }
.chosen > p { margin: 0.5rem 0 0; }
.grow { flex: 1; }
.summary { color: var(--text-2); }
.summary .short { display: none; }
.undo { display: inline-flex; gap: 0.5rem; align-items: center; padding-left: 0.75rem; border-left: 1px solid var(--border); }
/* One row on a phone: the short summary, or the reason Next waits, or the undo while it lasts. */
@media (max-width: 559px) {
  .summary .long { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
  .summary .short { display: inline; }
  .footer > .summary.has-hint, .footer > .has-undo.has-undo { display: none; }
  .footer > .undo.undo { display: flex; flex-wrap: wrap; gap: 0.25rem 0.5rem; padding-left: 0; border-left: 0; -webkit-line-clamp: none; line-clamp: none; }
}
</style>
