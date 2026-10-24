<script setup lang="ts" generic="T extends string">
// A row of toggle chips with counts (docs/design/01-design-system.md, "Components"). `All` clears
// the selection; several chips combine with OR.
import type { IconName } from "@/lib/icons";
import { useFmt } from "@/composables/useFmt";
import AppIcon from "./AppIcon.vue";

const props = defineProps<{
  options: { value: T; label: string; count?: number; icon?: IconName; color?: string }[];
  label: string;
  allLabel?: string;
  allCount?: number;
}>();
const model = defineModel<T[]>({ required: true });
const fmt = useFmt();

function toggle(v: T) {
  model.value = model.value.includes(v) ? model.value.filter((x) => x !== v) : [...model.value, v];
}
void props;
</script>

<template>
  <div class="chips" role="group" :aria-label="label">
    <button v-if="allLabel" type="button" class="chip" :aria-pressed="model.length === 0" @click="model = []">
      {{ allLabel }}<span v-if="allCount != null" class="count num">{{ fmt.count(allCount) }}</span>
    </button>
    <button v-for="o in options" :key="o.value" type="button" class="chip" :class="{ empty: o.count === 0 }" :aria-pressed="model.includes(o.value)" @click="toggle(o.value)">
      <AppIcon v-if="o.icon" :name="o.icon" :style="o.color ? { color: o.color } : undefined" />
      {{ o.label }}<span v-if="o.count != null" class="count num">{{ fmt.count(o.count) }}</span>
    </button>
  </div>
</template>

<style scoped>
.chips { display: flex; flex-wrap: wrap; gap: 0.4rem; }
.chip { font: inherit; font-size: var(--fs-sm); display: inline-flex; gap: 0.4rem; align-items: center; height: 1.9rem; padding: 0 0.7rem; border-radius: var(--radius-pill); border: 1px solid var(--border); background: transparent; color: var(--text-2); cursor: pointer; white-space: nowrap; }
.chip:hover { color: var(--text); background: var(--surface-2); border-color: var(--border-strong); }
.chip[aria-pressed="true"] { background: var(--accent-soft); border-color: var(--accent); color: var(--accent); font-weight: 600; }
.chip:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
/* The count in its own small badge, so "Active 3" reads as a label and a number. */
.count { font-size: var(--fs-xs); font-weight: 600; min-width: 1.3rem; padding: 0.05rem 0.4rem; border-radius: var(--radius-pill); background: var(--surface-2); color: var(--text-2); text-align: center; }
/* A group with nothing in it stays clickable but steps back. */
.chip.empty:not([aria-pressed="true"]) { opacity: 0.6; }
.chip[aria-pressed="true"] .count { background: var(--accent); color: var(--on-accent); }
</style>
