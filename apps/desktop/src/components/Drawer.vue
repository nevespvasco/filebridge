<script setup lang="ts">
// A side panel over the page (docs/design/01-design-system.md, "Components"): focus moves in and
// stays in, Esc or the backdrop closes it, and focus goes back where it was.
import { ref } from "vue";
import { useFocusTrap } from "@/composables/useFocusTrap";
import AppIcon from "./AppIcon.vue";

const props = withDefaults(defineProps<{ open: boolean; title: string; width?: number }>(), { width: 440 });
const emit = defineEmits<{ close: [] }>();
const panel = ref<HTMLElement | null>(null);
const { onKey } = useFocusTrap(
  panel,
  () => props.open,
  () => emit("close"),
);
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
.head h2 { font-size: var(--fs-base); margin: 0; }
.body { overflow: auto; padding: 1rem; display: grid; gap: 1rem; align-content: start; }
@keyframes slide { from { transform: translateX(24px); opacity: 0; } to { transform: none; opacity: 1; } }
/* A narrow window: the whole width (docs/design/02, "Window sizes"). */
@media (max-width: 559px) { .drawer { width: 100vw !important; border-left: 0; } }
@media (prefers-reduced-motion: reduce) { .drawer { animation: none; } }
</style>
