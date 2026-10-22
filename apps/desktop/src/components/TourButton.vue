<script setup lang="ts">
// The "?" next to a page title: plays its guide again.
import type { TourId } from "@/lib/tour";
import { useTour } from "@/stores/tour";
import AppIcon from "./AppIcon.vue";

const props = defineProps<{ tour: TourId }>();
const guides = useTour();
</script>

<template>
  <button class="tour-btn" type="button" :title="$t('tour.replay')" :aria-label="$t('tour.replay')" @click="guides.start(props.tour)">
    <AppIcon name="help" :size="18" />
  </button>
</template>

<style scoped>
.tour-btn {
  all: unset;
  box-sizing: border-box;
  display: inline-grid;
  place-items: center;
  width: 1.9rem;
  height: 1.9rem;
  border-radius: 50%;
  color: var(--text-2);
  border: 1px solid var(--border);
  cursor: pointer;
  vertical-align: middle;
}
.tour-btn:hover { background: var(--surface-2); color: var(--accent); }
.tour-btn:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
</style>
