<script setup lang="ts">
// "What to do now", or why something waits (docs/design/01-design-system.md, "Notices"): an icon
// and a sentence or two, in the tone of what it says. The look is the global `.guide` class.
import type { IconName } from "@/lib/icons";
import AppIcon from "./AppIcon.vue";

withDefaults(defineProps<{ tone?: "info" | "ok" | "warn"; icon?: IconName }>(), { tone: "info", icon: "pointing" });
</script>

<template>
  <div class="guide" :class="tone === 'info' ? '' : tone">
    <AppIcon class="guide-icon" :name="icon" :size="20" />
    <div class="notice-body"><slot /></div>
  </div>
</template>

<style scoped>
.notice-body { display: grid; gap: var(--sp-1); min-width: 0; }
</style>
