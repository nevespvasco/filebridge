<script setup lang="ts">
// Covers the window while files are dragged over it: the whole window takes the drop.
import AppIcon from "./AppIcon.vue";

defineProps<{ show: boolean }>();
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div v-if="show" class="drop-overlay" aria-hidden="true">
        <div class="frame">
          <AppIcon name="drop" :size="48" />
          <p>{{ $t("source.overlay") }}</p>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.drop-overlay { position: fixed; inset: 0; z-index: 70; pointer-events: none; background: var(--overlay); display: grid; padding: 1rem; }
.frame {
  border: 3px dashed var(--accent);
  border-radius: 16px;
  background: color-mix(in srgb, var(--accent-soft) 70%, transparent);
  display: grid;
  place-content: center;
  justify-items: center;
  gap: 0.5rem;
  color: var(--accent);
}
.frame p { margin: 0; font-size: 1.25rem; font-weight: 600; color: var(--text); }
.fade-enter-active, .fade-leave-active { transition: opacity 0.12s; }
.fade-enter-from, .fade-leave-to { opacity: 0; }
@media (prefers-reduced-motion: reduce) { .fade-enter-active, .fade-leave-active { transition: none; } }
</style>
