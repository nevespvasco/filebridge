<script setup lang="ts">
// One icon. Decorative by default (hidden from screen readers); with `label` it is announced.
import { computed } from "vue";
import { ICONS, type IconName } from "@/lib/icons";

const props = withDefaults(
  defineProps<{ name: IconName; size?: number; weight?: "regular" | "fill" | "bold"; label?: string }>(),
  { size: 16, weight: "regular", label: undefined },
);
const component = computed(() => ICONS[props.name]);
</script>

<template>
  <component
    :is="component"
    class="app-icon"
    :size="size"
    :weight="weight"
    :role="label ? 'img' : undefined"
    :aria-label="label"
    :aria-hidden="label ? undefined : 'true'"
    focusable="false"
  />
</template>

<style scoped>
.app-icon { flex: none; vertical-align: -0.15em; }
</style>
