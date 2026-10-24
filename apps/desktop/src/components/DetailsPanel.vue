<script setup lang="ts">
// The "Details" panel every step and job has (docs/06): the equivalent requests or commands, for
// transparency and debugging. Never shows secrets. Only with "Show technical information" on.
import { useSettings } from "@/stores/settings";

defineProps<{ lines: string[]; note?: string }>();
const settings = useSettings();
</script>

<template>
  <details v-if="settings.technical" class="details-panel">
    <summary>{{ $t("details.title") }} <span class="small muted hint">{{ $t("details.hint") }}</span></summary>
    <p v-if="note" class="muted small">{{ note }}</p>
    <pre v-if="lines.length"><code>{{ lines.join("\n") }}</code></pre>
    <p v-else class="muted small">{{ $t("details.none") }}</p>
  </details>
</template>

<style scoped>
.details-panel {
  margin-top: 1rem;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: 0.5rem 0.75rem;
  background: var(--surface-2);
}
summary {
  cursor: pointer;
  font-weight: 600;
  color: var(--text-2);
}
.hint { font-weight: 400; }
pre {
  margin: 0.5rem 0 0;
  overflow-x: auto;
  font-size: var(--fs-xs);
  line-height: 1.5;
  white-space: pre;
}
</style>
