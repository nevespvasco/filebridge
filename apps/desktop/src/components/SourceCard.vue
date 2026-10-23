<script setup lang="ts">
// One card of the quick-source grids in step 1 (docs/design/specs/D02): recently sent, pinned,
// disks. The card's main button adds its path; a side action (pin, unpin, whole disk) sits
// beside it, never inside it. A source that isn't there is dimmed and can't be added.
import type { IconName } from "@/lib/icons";
import AppIcon from "./AppIcon.vue";

withDefaults(
  defineProps<{ name: string; parent?: string; icon: IconName; disabled?: boolean; tip: string; action?: IconName }>(),
  { parent: undefined, action: "plus" },
);
const emit = defineEmits<{ add: [] }>();
</script>

<template>
  <div class="source-card" :class="{ disabled }">
    <button class="main" type="button" :aria-disabled="disabled" :title="tip" @click="!disabled && emit('add')">
      <span class="icon"><AppIcon :name="icon" :size="20" /></span>
      <span class="text">
        <span class="name">{{ name }}</span>
        <span v-if="parent" class="parent small muted">{{ parent }}</span>
        <span class="meta small muted"><slot /></span>
      </span>
      <AppIcon v-if="!disabled" class="plus" :name="action" />
    </button>
    <slot name="side" />
  </div>
</template>

<style scoped>
.source-card {
  display: flex;
  align-items: stretch;
  min-width: 0;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface);
  transition: border-color 0.12s, background 0.12s;
}
.source-card:hover:not(.disabled) { border-color: var(--accent); background: var(--accent-soft); }
.source-card.disabled { opacity: 0.55; border-style: dashed; }
.main {
  font: inherit;
  text-align: left;
  display: flex;
  gap: 0.65rem;
  align-items: center;
  flex: 1;
  min-width: 0;
  padding: 0.65rem 0.75rem;
  border: 0;
  border-radius: var(--radius-md);
  background: none;
  color: var(--text);
  cursor: pointer;
}
.main[aria-disabled="true"] { cursor: not-allowed; }
.main:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.icon { display: grid; place-items: center; width: 2.2rem; height: 2.2rem; flex: none; border-radius: var(--radius-sm); background: var(--surface-2); color: var(--accent); }
.text { display: grid; min-width: 0; flex: 1; }
.text > span { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.name { font-weight: 600; }
.plus { color: var(--text-2); flex: none; }
:slotted(.side) {
  font: inherit;
  display: grid;
  place-items: center;
  flex: none;
  min-width: 2.4rem;
  padding: 0 0.5rem;
  border: 0;
  border-left: 1px solid var(--border);
  border-radius: 0 var(--radius-md) var(--radius-md) 0;
  background: none;
  color: var(--text-2);
  cursor: pointer;
}
:slotted(.side:hover) { color: var(--accent); }
:slotted(.side[aria-pressed="true"]) { color: var(--accent); }
:slotted(.side:focus-visible) { outline: 2px solid var(--focus); outline-offset: -2px; }
@media (prefers-reduced-motion: reduce) { .source-card { transition: none; } }
</style>
