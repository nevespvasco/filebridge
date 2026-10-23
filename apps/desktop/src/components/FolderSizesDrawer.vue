<script setup lang="ts">
// "Sizes" of a chosen folder (docs/design/specs/D02): how its size splits between what is
// directly inside it, biggest first. The service counts (read-only), the numbers grow while it
// does (`folder_sizes_progress`), and it stops when asked, when another folder is opened, or
// when the panel closes. A folder inside opens in turn; the breadcrumbs go back.
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { FolderSizes_Serialize } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { api, errorMessage, events } from "@/lib/api";
import { baseName } from "@/lib/format";
import AppIcon from "./AppIcon.vue";
import Breadcrumbs from "./Breadcrumbs.vue";
import Drawer from "./Drawer.vue";

const props = defineProps<{ path: string | null }>();
const emit = defineEmits<{ close: [] }>();
const fmt = useFmt();
const { t } = useI18n();

/** The folders opened below `path`, in order. */
const trail = ref<{ name: string; path: string }[]>([]);
const current = computed(() => trail.value.at(-1)?.path ?? props.path);
const sizes = shallowRef<FolderSizes_Serialize | null>(null);
const running = ref(false);
const error = ref<string | null>(null);
let scanId: string | null = null;

function newId(): string {
  return `sizes-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

async function stop() {
  const id = scanId;
  if (id && running.value) {
    try {
      await api.cancelFolderSizes(id);
    } catch {
      // It ended meanwhile, or the service is gone: nothing to stop.
    }
  }
}

async function measure() {
  await stop();
  const path = current.value;
  if (!path) return;
  const id = newId();
  scanId = id;
  sizes.value = null;
  error.value = null;
  running.value = true;
  try {
    const answer = await api.folderSizes(id, path);
    if (scanId === id) sizes.value = answer;
  } catch (e) {
    if (scanId === id) error.value = errorMessage(e);
  } finally {
    if (scanId === id) running.value = false;
  }
}

let unlisten: (() => void) | null = null;
onMounted(async () => {
  unlisten = await events.serviceEvent.listen((e) => {
    const ev = e.payload;
    if (ev.type === "folder_sizes_progress" && ev.scan_id === scanId && running.value) sizes.value = ev.sizes;
  });
});
onBeforeUnmount(() => {
  unlisten?.();
  void stop();
});

watch(
  () => props.path,
  (path) => {
    trail.value = [];
    if (path) void measure();
    else {
      void stop();
      scanId = null;
      sizes.value = null;
      running.value = false;
    }
  },
  { immediate: true },
);

function open(name: string, path: string) {
  trail.value = [...trail.value, { name, path }];
  void measure();
}
function goTo(depth: number) {
  trail.value = trail.value.slice(0, depth);
  void measure();
}

/** Bars are relative to the biggest item, so the small ones still show. */
const biggest = computed(() => Math.max(1, ...(sizes.value?.items.map((i) => i.bytes) ?? [])));
const status = computed(() => {
  const s = sizes.value;
  if (!s) return running.value ? t("common.loading") : "";
  const args = { files: fmt.count(s.files), size: fmt.bytes(s.bytes) };
  if (running.value) return t("sourceTools.sizesCounting", args);
  return s.complete ? t("sourceTools.sizesDone", args) : t("sourceTools.sizesStopped", args);
});
</script>

<template>
  <Drawer :open="!!path" :title="$t('sourceTools.sizesTitle', { name: path ? baseName(path) : '' })" :width="520" @close="emit('close')">
    <div class="sizes">
      <p class="small muted lead">{{ $t("sourceTools.sizesLead") }}</p>
      <Breadcrumbs
        v-if="trail.length"
        :root="path ? baseName(path) : ''"
        :segments="trail.map((s) => s.name)"
        :label="$t('sourceTools.sizesCrumbs')"
        @go="goTo"
      />
      <div class="status">
        <span class="small num" aria-live="polite">{{ status }}</span>
        <span class="grow" />
        <button v-if="running" class="btn btn-small" type="button" @click="stop"><AppIcon name="stop" /> {{ $t("sourceTools.sizesStop") }}</button>
        <button v-else-if="sizes || error" class="btn btn-small" type="button" @click="measure"><AppIcon name="retry" /> {{ $t("sourceTools.sizesAgain") }}</button>
      </div>
      <p v-if="error" class="small error" role="alert">{{ $t("sourceTools.sizesFailed", { error }) }}</p>
      <p v-else-if="sizes && sizes.complete && !sizes.items.length" class="small muted">{{ $t("sourceTools.sizesEmpty") }}</p>
      <ul v-if="sizes?.items.length" class="items">
        <li v-for="i in sizes.items" :key="i.name">
          <component
            :is="i.is_dir && i.path ? 'button' : 'div'"
            class="row"
            :type="i.is_dir && i.path ? 'button' : undefined"
            :title="i.is_dir && i.path ? $t('sourceTools.sizesOpen', { name: i.name }) : undefined"
            @click="i.is_dir && i.path && open(i.name, i.path)"
          >
            <AppIcon :name="i.is_dir ? 'folder' : 'file'" class="kind" />
            <span class="name">{{ i.name }}</span>
            <span class="small muted num files">{{ i.is_dir ? $t("sourceTools.sizesFiles", { n: fmt.count(i.files) }, i.files) : "" }}</span>
            <span class="num bytes">{{ fmt.bytes(i.bytes) }}</span>
            <span class="bar" aria-hidden="true"><span :style="{ width: `${(i.bytes / biggest) * 100}%` }" /></span>
            <AppIcon v-if="i.is_dir && i.path" name="caretRight" class="go" />
          </component>
        </li>
        <li v-if="sizes.rest_items" class="rest small muted">
          {{ $t("sourceTools.sizesRest", { n: fmt.count(sizes.rest_items), size: fmt.bytes(sizes.rest_bytes) }, sizes.rest_items) }}
        </li>
      </ul>
      <p v-if="sizes?.errors" class="small warn-text">{{ $t("sourceTools.sizesErrors", { n: fmt.count(sizes.errors) }, sizes.errors) }}</p>
    </div>
  </Drawer>
</template>

<style scoped>
.sizes { display: grid; gap: 0.75rem; }
.sizes p { margin: 0; }
.status { display: flex; gap: 0.5rem; align-items: center; min-height: 2rem; }
.grow { flex: 1; }
.items { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.15rem; }
.row {
  font: inherit;
  color: inherit;
  text-align: left;
  width: 100%;
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto auto 1rem;
  grid-template-areas: "kind name files bytes go" "kind bar bar bar go";
  gap: 0.15rem 0.6rem;
  align-items: center;
  padding: 0.45rem 0.5rem;
  border: 0;
  border-radius: var(--radius-sm);
  background: none;
}
button.row { cursor: pointer; }
button.row:hover { background: var(--surface-2); }
button.row:focus-visible { outline: 2px solid var(--focus); outline-offset: 1px; }
.kind { grid-area: kind; color: var(--accent); }
.name { grid-area: name; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.files { grid-area: files; white-space: nowrap; }
.bytes { grid-area: bytes; white-space: nowrap; }
.go { grid-area: go; color: var(--text-2); }
.bar { grid-area: bar; height: 0.35rem; border-radius: 999px; background: var(--track); overflow: hidden; }
.bar > span { display: block; height: 100%; min-width: 2px; background: var(--accent); }
.rest { padding: 0.45rem 0.5rem; }
</style>
