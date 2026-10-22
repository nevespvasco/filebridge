<script setup lang="ts">
// Drop files and folders on the window, or pick them in the file explorer. `compact` is the
// one-line bar shown once something is chosen. On a phone: folders only, picked one at a time.
import AppIcon from "@/components/AppIcon.vue";
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { isMobile, pickPaths } from "@/lib/platform";
import { errorMessage } from "@/lib/api";
import { useFileDrop } from "@/composables/useFileDrop";
import DropOverlay from "./DropOverlay.vue";

defineProps<{ compact?: boolean }>();
const emit = defineEmits<{ add: [paths: string[]] }>();
const { t } = useI18n();
const error = ref<string | null>(null);
const { over } = useFileDrop((paths) => emit("add", paths));
const mobile = isMobile();

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
  <div class="drop" :class="{ over, compact }">
    <template v-if="compact">
      <AppIcon class="icon" name="drop" :size="22" />
      <span class="lead">{{ mobile ? $t("mobile.addMore") : $t("source.dropMore") }}</span>
      <span class="buttons">
        <button class="btn btn-small" :title="$t('source.pickFoldersTip')" @click="pick(true)"><AppIcon name="addFolder" /> {{ $t("source.addFolders") }}</button>
        <button v-if="!mobile" class="btn btn-small" :title="$t('source.pickFilesTip')" @click="pick(false)"><AppIcon name="addFile" /> {{ $t("source.addFiles") }}</button>
      </span>
    </template>
    <template v-else>
      <AppIcon class="icon" name="drop" :size="44" />
      <p v-if="mobile" class="lead">{{ $t("mobile.pickLead") }}</p>
      <p v-else class="lead">{{ $t("source.drop") }}<span class="or">{{ $t("source.dropOr") }}</span></p>
      <div class="buttons">
        <button class="btn btn-primary" :title="$t('source.pickFoldersTip')" @click="pick(true)"><AppIcon name="addFolder" /> {{ mobile ? $t("mobile.pickFolder") : $t("source.pickFolders") }}</button>
        <button v-if="!mobile" class="btn" :title="$t('source.pickFilesTip')" @click="pick(false)"><AppIcon name="addFile" /> {{ $t("source.pickFiles") }}</button>
      </div>
      <p class="hint small muted">{{ mobile ? $t("mobile.pickHint") : $t("source.hint") }}</p>
    </template>
    <p v-if="error" class="error small" role="alert">{{ error }}</p>
    <DropOverlay :show="over" />
  </div>
</template>

<style scoped>
.drop {
  border: 2px dashed var(--border-strong);
  border-radius: 12px;
  padding: 2.25rem 1rem;
  text-align: center;
  background: var(--surface-2);
  transition: border-color 0.15s, background 0.15s;
}
.drop.over { border-color: var(--accent); background: var(--accent-soft); }
.icon { color: var(--accent); }
.lead { margin: 0.5rem 0 1rem; color: var(--text-2); display: grid; gap: 0.35rem; }
.or { font-size: 0.85rem; }
.buttons { display: flex; gap: 0.5rem; justify-content: center; flex-wrap: wrap; }
.hint { margin: 0.9rem 0 0; }
.drop.compact {
  display: flex;
  align-items: center;
  gap: 0.6rem;
  padding: 0.55rem 0.8rem;
  text-align: left;
  border-width: 1.5px;
  flex-wrap: wrap;
}
.compact .lead { margin: 0; flex: 1; min-width: 10rem; display: block; }
.compact .buttons { justify-content: flex-end; }
.compact .error { width: 100%; margin: 0; }
@media (prefers-reduced-motion: reduce) { .drop { transition: none; } }
</style>
