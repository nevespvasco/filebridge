<script setup lang="ts">
// A side panel over the page (docs/design/01-design-system.md, "Components"): focus moves in and
// stays in, Esc or the backdrop closes it, and focus goes back where it was.
import { nextTick, onBeforeUnmount, ref, watch } from "vue";
import AppIcon from "./AppIcon.vue";

const props = withDefaults(defineProps<{ open: boolean; title: string; width?: number }>(), { width: 440 });
const emit = defineEmits<{ close: [] }>();
const panel = ref<HTMLElement | null>(null);
let returnTo: HTMLElement | null = null;

watch(
  () => props.open,
  async (open) => {
    if (open) {
      returnTo = document.activeElement as HTMLElement | null;
      await nextTick();
      panel.value?.focus();
    } else {
      returnTo?.focus?.();
    }
  },
  { immediate: true },
);

const FOCUSABLE = 'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';
function onKey(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.stopPropagation();
    emit("close");
    return;
  }
  if (e.key !== "Tab" || !panel.value) return;
  const items = [...panel.value.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => !el.hasAttribute("disabled"));
  if (!items.length) return;
  const [first, last] = [items[0], items[items.length - 1]];
  if (e.shiftKey && document.activeElement === first) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && document.activeElement === last) {
    e.preventDefault();
    first.focus();
  }
}
onBeforeUnmount(() => returnTo?.focus?.());
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="backdrop" @click.self="emit('close')">
      <aside ref="panel" class="drawer" role="dialog" aria-modal="true" :aria-label="title" tabindex="-1" :style="{ width: `min(${width}px, 100vw)` }" @keydown="onKey">
        <header class="head">
          <h2>{{ title }}</h2>
          <button type="button" class="btn btn-small" :aria-label="$t('common.close')" @click="emit('close')"><AppIcon name="close" /></button>
        </header>
        <div class="body"><slot /></div>
      </aside>
    </div>
  </Teleport>
</template>

<style scoped>
.backdrop { position: fixed; inset: 0; background: var(--overlay); z-index: 50; display: flex; justify-content: flex-end; }
.drawer { height: 100%; background: var(--surface); box-shadow: var(--shadow-pop); display: flex; flex-direction: column; outline: none; animation: slide 0.2s ease; border-left: 1px solid var(--border); }
.head { display: flex; align-items: center; justify-content: space-between; gap: 1rem; padding: 0.9rem 1rem; border-bottom: 1px solid var(--border); }
.head h2 { font-size: 1rem; margin: 0; }
.body { overflow: auto; padding: 1rem; display: grid; gap: 1rem; align-content: start; }
@keyframes slide { from { transform: translateX(24px); opacity: 0; } to { transform: none; opacity: 1; } }
@media (prefers-reduced-motion: reduce) { .drawer { animation: none; } }
</style>
