<script setup lang="ts">
// A list or panel that couldn't load (docs/design/01-design-system.md, "States"). With the copy
// engine down, the app-wide banner already says why and offers Try again, so this only says the
// content comes back once it answers. Otherwise: the problem in plain words, Try again, and the
// service's own message folded away.
import { useConnection } from "@/stores/connection";
import AppIcon from "./AppIcon.vue";
import EmptyState from "./EmptyState.vue";

withDefaults(defineProps<{ message: string; error?: string | null; compact?: boolean }>(), { error: null, compact: false });
defineEmits<{ retry: [] }>();
const conn = useConnection();
</script>

<template>
  <template v-if="conn.offline">
    <p v-if="compact" class="small muted waiting" role="status"><AppIcon name="activity" /> {{ $t("load.waitingShort") }}</p>
    <EmptyState v-else icon="activity" :title="$t('load.waitingTitle')" :hint="$t('load.waitingHint')" role="status" />
  </template>
  <div v-else class="load-error" :class="{ card: !compact }" role="alert">
    <p class="line" :class="{ small: compact }">
      <AppIcon name="warning" class="error" />
      <span>{{ message }}</span>
      <button class="btn btn-small" @click="$emit('retry')">{{ $t("common.retry") }}</button>
    </p>
    <details v-if="error" class="small muted">
      <summary>{{ $t("conn.details") }}</summary>
      <code>{{ error }}</code>
    </details>
  </div>
</template>

<style scoped>
.waiting { display: flex; gap: var(--sp-2); align-items: center; margin: 0; }
.load-error { display: grid; gap: var(--sp-2); }
.load-error.card { padding: var(--sp-4); }
.line { display: flex; flex-wrap: wrap; gap: var(--sp-2); align-items: center; margin: 0; }
.line > span { flex: 1 1 12rem; min-width: 0; }
summary { cursor: pointer; }
code { display: block; margin-top: var(--sp-1); overflow-wrap: anywhere; }
</style>
