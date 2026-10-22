<script setup lang="ts">
// Pick a folder at a destination with breadcrumbs (docs/design/specs/D11-destination-browser.md).
// Listing only reads: a "new folder" is a pending part of the path, created by the send itself.
import { trimBoth } from "@/lib/format";
import { computed, nextTick, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { BrowseResult, BrowseTarget_Deserialize } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { api, errorMessage } from "@/lib/api";
import AppIcon from "./AppIcon.vue";
import Breadcrumbs from "./Breadcrumbs.vue";
import RelativeTime from "./RelativeTime.vue";

const props = defineProps<{
  /** What to browse; `null` = closed. */
  target: BrowseTarget_Deserialize | null;
  title: string;
  /** Where to start, relative to the destination root. */
  start?: string | null;
  /** Without it, the dialog only looks (no "Choose"). */
  chooseLabel?: string;
  /** Offer "New folder" (a pending part of the path). Off where only existing folders make
   *  sense, like the folder to repair. Default: on when choosing. */
  allowNew?: boolean;
}>();
const emit = defineEmits<{ choose: [path: string]; close: [] }>();
const { t } = useI18n();
const fmt = useFmt();
void fmt;

const result = shallowRef<BrowseResult | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
const filter = ref("");
const pending = ref<string[]>([]);
const cache = new Map<string, BrowseResult>();
const active = ref(0);
const list = ref<HTMLElement | null>(null);

async function open(dir: string, refresh = false) {
  if (!props.target) return;
  pending.value = [];
  filter.value = "";
  active.value = 0;
  if (!refresh && cache.has(dir)) {
    result.value = cache.get(dir)!;
    return;
  }
  loading.value = true;
  error.value = null;
  try {
    const r = await api.browse(props.target, dir || null, null);
    cache.set(dir, r);
    result.value = r;
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    loading.value = false;
  }
}
watch(
  () => props.target,
  (t) => {
    cache.clear();
    result.value = null;
    if (t) void open(trimBoth(props.start ?? "", "/"));
  },
  { immediate: true },
);

const here = computed(() => result.value?.path ?? "");
const shown = computed(() => {
  const f = filter.value.trim().toLowerCase();
  return (result.value?.folders ?? []).filter((x) => !f || x.name.toLowerCase().includes(f));
});
/** More than the service listed: ask it again with the typed start of the name. */
async function narrow() {
  if (!props.target || !result.value?.truncated || !filter.value.trim()) return;
  try {
    result.value = await api.browse(props.target, here.value || null, filter.value.trim());
  } catch (e) {
    error.value = errorMessage(e);
  }
}
function go(depth: number) {
  const segs = result.value?.segments ?? [];
  void open(segs.slice(0, depth).join("/"));
}
const chosen = computed(() => [here.value, ...pending.value].filter(Boolean).join("/"));

// ---- new folder (checked against the destination's name rules; created by the send) ----
const naming = ref(false);
const newName = ref("");
const nameProblem = ref<string | null>(null);
const proposal = ref<string | null>(null);
async function addFolder() {
  if (!props.target) return;
  const name = newName.value.trim();
  if (!name) return;
  try {
    const c = await api.checkFolderName(props.target, name);
    if (!c.ok) {
      nameProblem.value = t("browse.badName", { name });
      proposal.value = c.proposed;
      return;
    }
    pending.value = [...pending.value, name];
    naming.value = false;
    newName.value = "";
    nameProblem.value = null;
    proposal.value = null;
  } catch (e) {
    nameProblem.value = errorMessage(e);
  }
}
function useProposal() {
  if (proposal.value) newName.value = proposal.value;
  nameProblem.value = null;
  proposal.value = null;
}

function onKey(e: KeyboardEvent) {
  const n = shown.value.length;
  if (e.key === "ArrowDown") active.value = Math.min(n - 1, active.value + 1);
  else if (e.key === "ArrowUp") active.value = Math.max(0, active.value - 1);
  else if (e.key === "Enter" && shown.value[active.value]) return void open(shown.value[active.value].path);
  else if (e.key === "Backspace" && (result.value?.segments.length ?? 0) > 0) return go((result.value?.segments.length ?? 1) - 1);
  else return;
  e.preventDefault();
  void nextTick(() => list.value?.querySelector<HTMLElement>(`[data-i="${active.value}"]`)?.scrollIntoView({ block: "nearest" }));
}
</script>

<template>
  <Teleport to="body">
    <div v-if="target" class="overlay" @click.self="emit('close')" @keydown.esc="emit('close')">
      <div class="dialog card" role="dialog" aria-modal="true" :aria-label="title">
        <h2>{{ title }}</h2>
        <Breadcrumbs :root="$t('browse.root')" :segments="result?.segments ?? []" :pending="pending" :label="$t('browse.where')" @go="go" />

        <div class="tools">
          <span class="small muted num">{{ $t("browse.count", { n: shown.length }, shown.length) }}</span>
          <input v-model="filter" class="filter" :placeholder="$t('browse.filter')" :aria-label="$t('browse.filter')" @keydown.enter="narrow" />
          <button class="btn btn-small" :disabled="loading" @click="open(here, true)"><AppIcon name="retry" /> {{ $t("common.refresh") }}</button>
        </div>
        <p v-if="result?.truncated" class="small muted">{{ $t("browse.truncated", { n: result.folders.length }) }}</p>
        <p v-if="error" class="error small" role="alert">{{ error }}</p>

        <ul
          ref="list"
          class="folders"
          role="listbox"
          tabindex="0"
          :aria-label="$t('browse.folders')"
          :aria-activedescendant="shown[active] ? `f-${active}` : undefined"
          :class="{ dim: loading }"
          @keydown="onKey"
        >
          <li v-if="!loading && !shown.length" class="empty small muted">{{ $t("browse.empty") }}</li>
          <li
            v-for="(f, i) in shown"
            :id="`f-${i}`"
            :key="f.path"
            :data-i="i"
            role="option"
            :aria-selected="i === active"
            :class="{ active: i === active }"
            @click="active = i"
            @dblclick="open(f.path)"
          >
            <AppIcon name="folder" />
            <span class="name">{{ f.name }}</span>
            <span v-if="f.mtime" class="small muted"><RelativeTime :at="f.mtime" /></span>
            <button class="btn btn-small open" :aria-label="$t('browse.open', { name: f.name })" @click.stop="open(f.path)"><AppIcon name="next" /></button>
          </li>
        </ul>

        <div v-if="chooseLabel && allowNew !== false" class="new">
          <button v-if="!naming" class="btn btn-small" @click="naming = true"><AppIcon name="addFolder" /> {{ $t("browse.newFolder") }}</button>
          <template v-else>
            <input v-model="newName" :placeholder="$t('browse.newName')" :aria-label="$t('browse.newName')" @keydown.enter="addFolder" />
            <button class="btn btn-small" @click="addFolder">{{ $t("common.add") }}</button>
            <button class="btn btn-small" @click="(naming = false), (nameProblem = null)">{{ $t("common.cancel") }}</button>
          </template>
          <p v-if="nameProblem" class="small error">
            {{ nameProblem }}
            <button v-if="proposal" class="link" @click="useProposal">{{ $t("browse.useProposal", { name: proposal }) }}</button>
          </p>
        </div>

        <p v-if="chooseLabel" class="into small">{{ $t("browse.into") }} <span class="path">/{{ chosen }}</span></p>
        <div class="actions">
          <button class="btn" @click="emit('close')">{{ chooseLabel ? $t("common.cancel") : $t("common.close") }}</button>
          <button v-if="chooseLabel" class="btn btn-primary" @click="emit('choose', chosen)">{{ chooseLabel }}</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.overlay { position: fixed; inset: 0; background: var(--overlay); display: grid; place-items: center; z-index: 50; padding: 1rem; }
.dialog { width: min(600px, 100%); display: grid; gap: 0.7rem; max-height: calc(100vh - 2rem); }
h2 { margin: 0; font-size: 1.1rem; }
.tools { display: flex; gap: 0.6rem; align-items: center; }
.filter { flex: 1; }
.folders { list-style: none; margin: 0; padding: 0; border: 1px solid var(--border); border-radius: var(--radius-sm); max-height: 18rem; overflow: auto; outline: none; }
.folders:focus-visible { box-shadow: 0 0 0 2px var(--focus); }
.folders li { display: flex; gap: 0.5rem; align-items: center; padding: 0.4rem 0.6rem; border-bottom: 1px solid var(--rule); cursor: pointer; }
.folders li:last-child { border-bottom: 0; }
.folders li.active { background: var(--accent-soft); }
.folders .name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.folders .empty { cursor: default; justify-content: center; padding: 1.2rem; }
.folders.dim { opacity: 0.6; }
.new { display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; }
.new input { flex: 1; min-width: 10rem; }
.new p { width: 100%; margin: 0; }
.link { all: unset; cursor: pointer; color: var(--accent); margin-left: 0.3rem; }
.into { margin: 0; }
.actions { display: flex; justify-content: flex-end; gap: 0.5rem; }
</style>
