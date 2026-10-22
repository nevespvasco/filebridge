<script setup lang="ts" generic="T extends string">
// A small set of mutually exclusive choices shown side by side (a radio group).
import type { IconName } from "@/lib/icons";
import AppIcon from "./AppIcon.vue";

defineProps<{ options: { value: T; label: string; icon?: IconName }[]; label: string }>();
const model = defineModel<T>({ required: true });
</script>

<template>
  <div class="segmented" role="radiogroup" :aria-label="label">
    <label v-for="o in options" :key="o.value" class="seg" :class="{ on: model === o.value }">
      <input v-model="model" class="sr-only" type="radio" :value="o.value" :name="label" />
      <AppIcon v-if="o.icon" :name="o.icon" />
      <span>{{ o.label }}</span>
    </label>
  </div>
</template>

<style scoped>
.segmented { display: inline-flex; justify-self: start; width: max-content; max-width: 100%; padding: 3px; gap: 3px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); background: var(--surface-2); }
.seg { display: inline-flex; gap: 0.35rem; align-items: center; padding: 0.3rem 0.75rem; border-radius: 4px; cursor: pointer; color: var(--text-2); }
.seg:hover { color: var(--text); }
.seg.on { background: var(--surface); color: var(--text); font-weight: 600; box-shadow: 0 0 0 1px var(--border); }
.seg:has(input:focus-visible) { outline: 2px solid var(--focus); outline-offset: 1px; }
</style>
