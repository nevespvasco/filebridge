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
    <button v-for="o in options" :key="o.value" type="button" class="chip" :aria-pressed="model.includes(o.value)" @click="toggle(o.value)">
      <AppIcon v-if="o.icon" :name="o.icon" :style="o.color ? { color: o.color } : undefined" />
      {{ o.label }}<span v-if="o.count != null" class="count num">{{ fmt.count(o.count) }}</span>
    </button>
  </div>
</template>

<style scoped>
.chips { display: flex; flex-wrap: wrap; gap: 0.4rem; }
.chip { font: inherit; font-size: var(--fs-sm); display: inline-flex; gap: 0.35rem; align-items: center; padding: 0.25rem 0.65rem; border-radius: 999px; border: 1px solid var(--border-strong); background: var(--surface); color: var(--text-2); cursor: pointer; }
.chip:hover { color: var(--text); background: var(--surface-2); }
.chip[aria-pressed="true"] { background: var(--accent-soft); border-color: var(--accent); color: var(--accent); font-weight: 600; }
.chip:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.count { color: inherit; opacity: 0.75; }
</style>
