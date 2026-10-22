<script setup lang="ts">
// Sticky at the bottom of its list while anything is selected (docs/design/01-design-system.md).
import { useFmt } from "@/composables/useFmt";

defineProps<{ count: number; busy?: boolean; actions: { id: string; label: string; primary?: boolean }[] }>();
const emit = defineEmits<{ act: [id: string]; clear: [] }>();
const fmt = useFmt();
</script>

<template>
  <div v-if="count > 0" class="selection-bar" role="region" :aria-label="$t('selection.label')">
    <strong class="num" aria-live="polite">{{ $t("selection.count", { n: fmt.count(count) }, count) }}</strong>
    <span class="grow" />
    <button v-for="a in actions" :key="a.id" class="btn btn-small" :class="{ 'btn-primary': a.primary }" :disabled="busy" @click="emit('act', a.id)">{{ a.label }}</button>
    <button class="btn btn-small" :disabled="busy" @click="emit('clear')">{{ $t("selection.clear") }}</button>
  </div>
</template>

<style scoped>
.selection-bar { position: sticky; bottom: 0.75rem; z-index: 5; display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; margin-top: 0.75rem; padding: 0.6rem 0.8rem; border-radius: var(--radius-md); background: var(--surface); border: 1px solid var(--accent); box-shadow: var(--shadow-pop); }
.grow { flex: 1; }
</style>
