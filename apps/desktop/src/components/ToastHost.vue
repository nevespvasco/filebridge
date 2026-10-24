<script setup lang="ts">
// Where toasts appear: bottom centre, above the phone tab bar; read out politely.
import { useToast } from "@/stores/toast";
import AppIcon from "./AppIcon.vue";

const toast = useToast();
</script>

<template>
  <div class="toast-host" role="status" aria-live="polite">
    <Transition name="toast">
      <div v-if="toast.current" :key="toast.current.id" class="toast" :class="`tone-${toast.current.tone}`">
        <AppIcon :name="toast.current.tone === 'ok' ? 'checkCircle' : 'warning'" />
        <span>{{ toast.current.text }}</span>
        <button type="button" class="close" :aria-label="$t('common.dismiss')" @click="toast.dismiss()"><AppIcon name="close" /></button>
      </div>
    </Transition>
  </div>
</template>

<style scoped>
.toast-host { position: fixed; left: 0; right: 0; bottom: calc(var(--nav-bottom) + var(--sp-4)); display: flex; justify-content: center; pointer-events: none; z-index: 60; padding: 0 var(--sp-4); }
.toast { pointer-events: auto; display: flex; gap: var(--sp-2); align-items: center; padding: var(--sp-2) var(--sp-3); border-radius: var(--radius-md); background: var(--text); color: var(--bg); box-shadow: var(--shadow-pop); font-size: var(--fs-sm); max-width: 32rem; }
.tone-ok :deep(.app-icon:first-child) { color: var(--ok); }
.tone-error :deep(.app-icon:first-child) { color: var(--danger); }
.close { all: unset; cursor: pointer; display: inline-flex; opacity: 0.7; }
.close:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.toast-enter-active, .toast-leave-active { transition: opacity 0.15s, transform 0.15s; }
.toast-enter-from, .toast-leave-to { opacity: 0; transform: translateY(6px); }
@media (prefers-reduced-motion: reduce) { .toast-enter-active, .toast-leave-active { transition: none; } }
</style>
