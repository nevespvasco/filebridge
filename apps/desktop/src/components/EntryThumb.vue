<script setup lang="ts">
// A file's Category icon, replaced by a preview of its source image once one has loaded (ADR
// 0027). Same box either way, so rows never shift. Asked for only after the row has stayed on
// screen a moment: a fast scroll past thousands of rows asks for none of them.
import { onBeforeUnmount, ref, watch } from "vue";
import type { FileCategory } from "@/bindings";
import { CATEGORY_ICON, categoryColor } from "@/lib/categories";
import { knownThumbnail, loadThumbnail } from "@/lib/thumbnails";
import AppIcon from "./AppIcon.vue";

const props = withDefaults(defineProps<{ jobId: string; idx: number; category: FileCategory; size?: number; preview?: boolean }>(), {
  size: 24,
  preview: true,
});

/** How long a row stays on screen before its preview is asked for. */
const SETTLE_MS = 150;

const src = ref<string | null>(null);
let timer: ReturnType<typeof setTimeout> | undefined;
let pending: { cancel: () => void } | undefined;

function stop() {
  clearTimeout(timer);
  pending?.cancel();
  timer = pending = undefined;
}

watch(
  () => [props.jobId, props.idx, props.category, props.preview] as const,
  ([jobId, idx, category, preview]) => {
    stop();
    src.value = null;
    if (!preview || category !== "image") return;
    const had = knownThumbnail(jobId, idx);
    if (had !== undefined) {
      src.value = had;
      return;
    }
    timer = setTimeout(() => {
      const req = loadThumbnail(jobId, idx);
      pending = req;
      void req.promise.then((url) => {
        if (pending === req) src.value = url;
      });
    }, SETTLE_MS);
  },
  { immediate: true },
);
onBeforeUnmount(stop);
</script>

<template>
  <span class="thumb" :style="{ width: `${size}px`, height: `${size}px` }">
    <img v-if="src" :src="src" alt="" aria-hidden="true" decoding="async" />
    <AppIcon v-else :name="CATEGORY_ICON[category]" :style="{ color: categoryColor(category) }" />
  </span>
</template>

<style scoped>
.thumb { flex: none; display: inline-grid; place-items: center; border-radius: 4px; overflow: hidden; }
.thumb img { width: 100%; height: 100%; object-fit: cover; }
</style>
