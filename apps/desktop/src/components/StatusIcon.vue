<script setup lang="ts">
// ✓ / ! / ✗ for `ok` / `warn` / `fail` steps and checks.
import { computed } from "vue";
import { useI18n } from "vue-i18n";

const props = defineProps<{ status: string }>();
const { t } = useI18n();
const glyph = computed(() => ({ ok: "✓", warn: "!", fail: "✗" })[props.status] ?? "•");
const label = computed(() => (["ok", "warn", "fail"].includes(props.status) ? t(`status.${props.status}`) : props.status));
</script>

<template>
  <span class="status-icon" :class="`status-${status}`" :title="label" :aria-label="label" role="img">{{ glyph }}</span>
</template>

<style scoped>
.status-icon {
  display: inline-grid;
  place-items: center;
  width: 1.4em;
  height: 1.4em;
  border-radius: 50%;
  font-weight: 700;
  font-size: 0.85em;
  flex: none;
  color: var(--on-accent);
  background: var(--muted);
}
.status-ok { background: var(--ok); }
.status-warn { background: var(--warn); color: #1f1600; }
.status-fail { background: var(--danger); }
</style>
