<script setup lang="ts">
defineProps<{ value?: number; indeterminate?: boolean; tone?: "accent" | "ok" | "warn" | "danger" }>();
</script>

<template>
  <div
    class="progress"
    role="progressbar"
    :aria-valuenow="indeterminate ? undefined : Math.round(value ?? 0)"
    aria-valuemin="0"
    aria-valuemax="100"
    :class="[{ indeterminate }, `tone-${tone ?? 'accent'}`]"
  >
    <div class="fill" :style="indeterminate ? undefined : { width: `${value ?? 0}%` }" />
  </div>
</template>

<style scoped>
.progress {
  height: 8px;
  border-radius: 4px;
  background: var(--track);
  overflow: hidden;
  position: relative;
}
.fill {
  height: 100%;
  background: var(--accent);
  transition: width 0.3s ease;
}
.tone-ok .fill { background: var(--ok); }
.tone-warn .fill { background: var(--warn); }
.tone-danger .fill { background: var(--danger); }
.indeterminate .fill {
  width: 30%;
  position: absolute;
  animation: slide 1.2s ease-in-out infinite;
}
@keyframes slide {
  from { left: -30%; }
  to { left: 100%; }
}
@media (prefers-reduced-motion: reduce) {
  .indeterminate .fill { animation: none; width: 100%; opacity: 0.4; }
}
</style>
