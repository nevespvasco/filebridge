<script setup lang="ts">
import type { EntryStatus } from "@/bindings";
import { STATUS_LOOK } from "@/lib/entries";
import AppIcon from "./AppIcon.vue";

defineProps<{ status: EntryStatus }>();
</script>

<template>
  <span class="badge" :class="`badge-${STATUS_LOOK[status].tone}`"><AppIcon :name="STATUS_LOOK[status].icon" :size="12" weight="bold" /> {{ $t(`entryStatus.${status}`) }}</span>
</template>

<style scoped>
.badge { display: inline-flex; gap: 0.3rem; align-items: center; }
</style>
