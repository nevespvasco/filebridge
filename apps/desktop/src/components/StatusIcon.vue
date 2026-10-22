<script setup lang="ts">
// A check / exclamation / cross in a coloured disc for `ok` / `warn` / `fail` steps and checks.
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { IconName } from "@/lib/icons";
import AppIcon from "./AppIcon.vue";

const props = defineProps<{ status: string }>();
const { t } = useI18n();
const ICON: Record<string, IconName> = { ok: "check", warn: "exclaim", fail: "close" };
const icon = computed<IconName>(() => ICON[props.status] ?? "pending");
const label = computed(() => (["ok", "warn", "fail"].includes(props.status) ? t(`status.${props.status}`) : props.status));
</script>

<template>
  <span class="status-icon" :class="`status-${status}`" :title="label" :aria-label="label" role="img"><AppIcon :name="icon" :size="12" weight="bold" /></span>
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
