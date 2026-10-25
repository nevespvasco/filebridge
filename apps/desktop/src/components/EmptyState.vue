<script setup lang="ts">
// "Nothing here yet", the same on every screen (docs/design/01-design-system.md, "Empty states"):
// an icon, one sentence, a hint, and at most one button (the slot).
import type { IconName } from "@/lib/icons";
import AppIcon from "./AppIcon.vue";

defineProps<{ icon: IconName; title: string; hint?: string }>();
</script>

<template>
  <div class="empty-state card">
    <AppIcon class="empty-icon" :name="icon" :size="40" />
    <strong>{{ title }}</strong>
    <p v-if="hint" class="small muted">{{ hint }}</p>
    <slot />
  </div>
</template>

<style scoped>
.empty-state { display: grid; justify-items: center; text-align: center; gap: var(--sp-2); padding: var(--sp-6) var(--sp-4); }
.empty-state p { margin: 0 0 var(--sp-2); max-width: 50ch; }
.empty-icon { color: var(--muted); }
</style>
