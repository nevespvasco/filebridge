<script setup lang="ts">
// A destination's space from its last check: used and free, its age, the tool's quota when
// there is one, and a warning when it runs low (docs/design/specs/D03-destination-storage.md).
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useFmt } from "@/composables/useFmt";
import { useProfiles } from "@/stores/profiles";
import AppIcon from "./AppIcon.vue";
import RelativeTime from "./RelativeTime.vue";
import StorageMeter from "./StorageMeter.vue";

const props = defineProps<{
  profileId: string;
  compact?: boolean;
  /** What "Check now" runs, when the screen does more than the check (e.g. Destinations offers
   *  to trust an unknown SSH server's key, then checks again). */
  refresh?: () => unknown;
}>();
const profiles = useProfiles();
const { t } = useI18n();
const fmt = useFmt();

const s = computed(() => profiles.storage[props.profileId]);
const checking = computed(() => !!profiles.checking[props.profileId]);
const known = computed(() => s.value?.free_bytes != null);
const percent = computed(() => (s.value?.used_bytes != null && s.value.total_bytes ? Math.round((s.value.used_bytes / s.value.total_bytes) * 100) : null));
const used = computed(() => (s.value?.used_bytes != null ? [{ key: "used", label: t("storage.usedLabel"), bytes: s.value.used_bytes, color: s.value.low ? "var(--danger)" : "var(--text-2)" }] : []));
const quota = computed(() => s.value?.quota);

function checkNow() {
  if (props.refresh) props.refresh();
  else void profiles.loadStorage(props.profileId, true);
}
</script>

<template>
  <div class="space" :class="{ compact }">
    <template v-if="!s || s.checked_at == null">
      <p class="small muted">
        {{ $t("storage.never") }}
        <button class="link small" :disabled="checking" @click="checkNow">{{ $t("storage.checkNow") }}</button>
      </p>
    </template>
    <template v-else-if="known">
      <StorageMeter
        v-if="s.total_bytes != null && used.length"
        :segments="used"
        :total="s.total_bytes"
        mode="usage"
        :caption="$t('storage.caption')"
        :legend="false"
        class="meter"
        :class="{ checking }"
      />
      <p class="line small">
        <span class="num">
          <template v-if="s.used_bytes != null && s.total_bytes != null">{{ $t("storage.used", { used: fmt.bytes(s.used_bytes), free: fmt.bytes(s.free_bytes), total: fmt.bytes(s.total_bytes) }) }}</template>
          <template v-else>{{ $t("storage.freeOnly", { free: fmt.bytes(s.free_bytes) }) }}</template>
          <template v-if="percent != null"> · {{ $t("storage.percent", { p: percent }) }}</template>
        </span>
        <span class="when" :class="{ stale: s.stale }">
          <AppIcon v-if="s.stale" name="waiting" :label="$t('storage.staleLabel')" />
          {{ $t("storage.checked") }} <RelativeTime :at="s.checked_at" />
          <button class="link" :disabled="checking" @click="checkNow">{{ checking ? $t("storage.checking") : $t("storage.checkNow") }}</button>
        </span>
      </p>
      <p v-if="quota && !compact" class="small muted">
        {{ $t("storage.quota", { used: fmt.bytes(quota.used_bytes), total: fmt.bytes(quota.total_bytes) }) }}
      </p>
      <p v-if="s.total_bytes == null && !compact" class="small muted">{{ $t("storage.noTotal") }}</p>
      <p v-if="s.low" class="notice small" role="status"><AppIcon name="warning" /> {{ $t("storage.low", { free: fmt.bytes(s.free_bytes) }) }}</p>
    </template>
    <p v-else class="small muted">{{ $t("storage.unsupported") }}</p>
    <p v-if="s?.last_error && !compact" class="small error">{{ $t("storage.lastError", { reason: s.last_error }) }}</p>
  </div>
</template>

<style scoped>
.space { display: grid; gap: 0.35rem; }
.meter.checking { opacity: 0.6; }
.line { display: flex; justify-content: space-between; gap: 0.5rem 1rem; flex-wrap: wrap; margin: 0; }
.when { color: var(--text-2); display: inline-flex; gap: 0.3rem; align-items: center; }
.when.stale { color: var(--warn-strong); }
.link { all: unset; cursor: pointer; color: var(--accent); margin-left: 0.3rem; }
.link:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; border-radius: 3px; }
.link:disabled { color: var(--muted); cursor: default; }
.notice { display: inline-flex; gap: 0.35rem; align-items: center; margin: 0; padding: 0.3rem 0.6rem; border-radius: var(--radius-sm); background: var(--warn-soft); color: var(--warn-strong); width: fit-content; }
p { margin: 0; }
</style>
