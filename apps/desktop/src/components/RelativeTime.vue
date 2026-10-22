<script setup lang="ts">
// "3 min ago" / "há 3 min", with the exact time on hover (docs/design/01-design-system.md).
import { computed, onBeforeUnmount, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useFmt } from "@/composables/useFmt";
import { bcp47 } from "@/i18n";

const props = defineProps<{ at: number }>();
const { locale } = useI18n();
const fmt = useFmt();

// Re-render every minute so "just now" ages.
const now = ref(Date.now());
const timer = setInterval(() => (now.value = Date.now()), 60_000);
onBeforeUnmount(() => clearInterval(timer));

const STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 86_400_000],
  ["month", 30 * 86_400_000],
  ["week", 7 * 86_400_000],
  ["day", 86_400_000],
  ["hour", 3_600_000],
  ["minute", 60_000],
];

const text = computed(() => {
  const diff = props.at - now.value;
  const rtf = new Intl.RelativeTimeFormat(bcp47(locale.value), { numeric: "auto", style: "short" });
  for (const [unit, ms] of STEPS) {
    if (Math.abs(diff) >= ms) return rtf.format(Math.round(diff / ms), unit);
  }
  return rtf.format(0, "minute");
});
</script>

<template>
  <time :datetime="new Date(at).toISOString()" :title="fmt.date(at)">{{ text }}</time>
</template>
