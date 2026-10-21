<script setup lang="ts">
import { computed } from "vue";
import type { JobState } from "@/bindings";

const props = defineProps<{ state: JobState }>();
const tone = computed(() => {
  switch (props.state) {
    case "completed":
      return "ok";
    case "completed_with_warnings":
    case "awaiting_approval":
    case "retrying":
    case "paused":
      return "warn";
    case "failed":
      return "danger";
    case "transferring":
    case "verifying":
    case "integrating":
    case "analysing":
      return "accent";
    default:
      return "muted";
  }
});
</script>

<template>
  <span class="badge" :class="`badge-${tone}`">{{ $t(`state.${state}`) }}</span>
</template>
