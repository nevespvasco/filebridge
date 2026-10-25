<script setup lang="ts">
// A message cut after `lines` lines, with "More" / "Less" when it is longer: an error that quotes
// a whole tool's answer stays one tap away without filling a small screen.
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

const props = withDefaults(defineProps<{ text: string; lines?: number }>(), { lines: 2 });
const el = ref<HTMLElement | null>(null);
const open = ref(false);
const overflows = ref(false);

async function measure() {
  if (open.value) return;
  await nextTick();
  const e = el.value;
  overflows.value = !!e && e.scrollHeight > e.clientHeight + 1;
}
// The cut moves with the width (a window resized, a card that reflows).
let observer: ResizeObserver | null = null;
onMounted(() => {
  void measure();
  if (el.value) {
    observer = new ResizeObserver(() => void measure());
    observer.observe(el.value);
  }
});
onBeforeUnmount(() => observer?.disconnect());
watch(
  () => props.text,
  () => {
    open.value = false;
    void measure();
  },
);
</script>

<template>
  <span class="clamp">
    <span ref="el" class="text" :class="{ cut: !open }" :style="{ '--lines': lines }">{{ text }}</span>
    <button v-if="overflows || open" class="link-btn" type="button" :aria-expanded="open" @click="open = !open">
      {{ open ? $t("common.less") : $t("common.more") }}
    </button>
  </span>
</template>

<style scoped>
.clamp { display: grid; justify-items: start; gap: 0.1rem; min-width: 0; }
.text { overflow-wrap: anywhere; }
.text.cut { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: var(--lines); line-clamp: var(--lines); overflow: hidden; }
</style>
