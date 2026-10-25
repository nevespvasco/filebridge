<script setup lang="ts">
// "3 min ago" / "há 3 min", with the exact time on hover (docs/design/01-design-system.md). One
// rule everywhere: minutes, then hours up to 23, then "yesterday" for the day before, then the
// date. The same moment never reads "24 h ago" in one place and "yesterday" in another.
import { computed, onBeforeUnmount, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useFmt } from "@/composables/useFmt";
import { bcp47 } from "@/i18n";
import { relativeTime } from "@/lib/format";

const props = defineProps<{ at: number }>();
const { locale } = useI18n();
const fmt = useFmt();

// Re-render every minute so "just now" ages.
const now = ref(Date.now());
const timer = setInterval(() => (now.value = Date.now()), 60_000);
onBeforeUnmount(() => clearInterval(timer));

const text = computed(() => relativeTime(props.at, now.value, bcp47(locale.value)));
</script>

<template>
  <time :datetime="new Date(at).toISOString()" :title="fmt.date(at)">{{ text }}</time>
</template>
