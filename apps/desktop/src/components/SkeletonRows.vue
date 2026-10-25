<script setup lang="ts">
// Grey placeholder lines while a list's first answer is on its way (docs/design/01-design-system.md,
// "States"): the page keeps its shape and never says "nothing here" before it knows. Still (no
// shimmer) when the user asks for less motion.
withDefaults(defineProps<{ rows?: number }>(), { rows: 3 });
</script>

<template>
  <div class="skeleton" role="status" :aria-label="$t('common.loading')">
    <div v-for="i in rows" :key="i" class="sk-row" aria-hidden="true">
      <span class="sk sk-name" />
      <span class="sk sk-mid" />
      <span class="sk sk-end" />
    </div>
  </div>
</template>

<style scoped>
.skeleton { display: grid; padding: var(--sp-2) var(--sp-4); }
.sk-row { display: grid; grid-template-columns: minmax(0, 2fr) minmax(0, 1fr) 4rem; gap: var(--sp-4); align-items: center; padding: var(--sp-3) 0; border-bottom: 1px solid var(--rule); }
.sk-row:last-child { border-bottom: 0; }
.sk { display: block; height: 0.8rem; border-radius: var(--radius-sm); background: var(--surface-2); animation: pulse 1.4s ease-in-out infinite; }
.sk-name { width: 70%; }
.sk-mid { width: 50%; }
@keyframes pulse { 50% { opacity: 0.55; } }
@media (prefers-reduced-motion: reduce) { .sk { animation: none; } }
</style>
