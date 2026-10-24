<script setup lang="ts">
// Where you are in a folder tree; each part goes back there (docs/design/01-design-system.md).
// Long paths fold their middle into "…".
import { computed } from "vue";

const props = defineProps<{ root: string; segments: string[]; label: string; pending?: string[] }>();
const emit = defineEmits<{ go: [depth: number] }>();

/** Index of each shown segment in `segments` (-1 = "…"). */
const shown = computed(() => {
  const n = props.segments.length;
  if (n <= 4) return props.segments.map((_, i) => i);
  return [0, -1, n - 2, n - 1];
});
</script>

<template>
  <nav class="crumbs" :aria-label="label">
    <ol>
      <li><button type="button" class="crumb" :aria-current="segments.length === 0 && !pending?.length ? 'page' : undefined" @click="emit('go', 0)">{{ root }}</button></li>
      <li v-for="i in shown" :key="i">
        <span class="sep" aria-hidden="true">›</span>
        <span v-if="i === -1" class="muted">…</span>
        <button v-else type="button" class="crumb" :aria-current="i === segments.length - 1 && !pending?.length ? 'page' : undefined" @click="emit('go', i + 1)">{{ segments[i] }}</button>
      </li>
      <li v-for="p in pending ?? []" :key="`p-${p}`">
        <span class="sep" aria-hidden="true">›</span>
        <em class="pending">{{ p }} <span class="small muted">{{ $t("browse.willCreate") }}</span></em>
      </li>
    </ol>
  </nav>
</template>

<style scoped>
.crumbs ol { list-style: none; display: flex; flex-wrap: wrap; gap: 0.15rem 0.3rem; margin: 0; padding: 0; align-items: center; }
.crumbs li { display: inline-flex; gap: 0.3rem; align-items: center; min-width: 0; }
.crumb { all: unset; cursor: pointer; color: var(--accent); max-width: 14rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.crumb[aria-current="page"] { color: var(--text); font-weight: 600; cursor: default; }
.crumb:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; border-radius: var(--radius-xs); }
.sep { color: var(--muted); }
.pending { color: var(--text-2); }
</style>
