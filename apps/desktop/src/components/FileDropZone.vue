<script setup lang="ts">
// Drop files and folders on the window, or pick them in the file explorer.
import AppIcon from "@/components/AppIcon.vue";
import { onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { onFileDrop, pickPaths } from "@/lib/platform";
import { errorMessage } from "@/lib/api";

const emit = defineEmits<{ add: [paths: string[]] }>();
const { t } = useI18n();
const over = ref(false);
const error = ref<string | null>(null);
let unlisten: (() => void) | null = null;

onMounted(async () => {
  unlisten = await onFileDrop((state, paths) => {
    over.value = state === "over";
    if (state === "drop" && paths.length) emit("add", paths);
  });
});
onBeforeUnmount(() => unlisten?.());

async function pick(directory: boolean) {
  error.value = null;
  try {
    const paths = await pickPaths({ directory, title: directory ? t("source.pickFolders") : t("source.pickFiles") });
    if (paths.length) emit("add", paths);
  } catch (e) {
    error.value = errorMessage(e);
  }
}
</script>

<template>
  <div class="drop" :class="{ over }">
    <AppIcon class="icon" name="drop" :size="40" />
    <p class="lead">{{ $t("source.drop") }}<span class="or">{{ $t("source.dropOr") }}</span></p>
    <div class="buttons">
      <button class="btn btn-primary" :title="$t('source.pickFoldersTip')" @click="pick(true)"><AppIcon name="addFolder" /> {{ $t("source.pickFolders") }}</button>
      <button class="btn" :title="$t('source.pickFilesTip')" @click="pick(false)"><AppIcon name="addFile" /> {{ $t("source.pickFiles") }}</button>
    </div>
    <p v-if="error" class="error small">{{ error }}</p>
  </div>
</template>

<style scoped>
.drop {
  border: 2px dashed var(--border-strong);
  border-radius: 12px;
  padding: 2rem 1rem;
  text-align: center;
  background: var(--surface-2);
  transition: border-color 0.15s, background 0.15s;
}
.drop.over { border-color: var(--accent); background: var(--accent-soft); }
.icon { color: var(--accent); }
.lead { margin: 0.5rem 0 1rem; color: var(--text-2); display: grid; gap: 0.35rem; }
.or { font-size: 0.85rem; }
.buttons { display: flex; gap: 0.5rem; justify-content: center; flex-wrap: wrap; }
</style>
