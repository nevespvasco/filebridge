<script setup lang="ts">
// Transport and integration preflight checks (free space, write access, tool checks), in plain
// words: "✓ Connection", "✓ Writing · 600 GB free". A check's own message shows when it isn't OK;
// its code only with "Show technical information" on.
import { useI18n } from "vue-i18n";
import type { PreflightView_Serialize } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { useSettings } from "@/stores/settings";
import StatusIcon from "./StatusIcon.vue";

defineProps<{ views: PreflightView_Serialize[] }>();
const fmt = useFmt();
const settings = useSettings();
const { t, te } = useI18n();
const label = (id: string) => (te(`preflight.check.${id}`) ? t(`preflight.check.${id}`) : null);
</script>

<template>
  <div class="preflight">
    <div v-for="v in views" :key="v.source" class="block">
      <div class="head">
        <strong>{{ $t(`preflight.${v.source}`) }}</strong>
        <span class="muted small">
          {{ v.free_bytes != null ? $t("preflight.free", { size: fmt.bytes(v.free_bytes) }) : $t("preflight.freeUnknown") }}
          · {{ v.writable ? $t("preflight.writable") : $t("preflight.notWritable") }}
        </span>
      </div>
      <ul class="checks">
        <li v-for="c in v.checks" :key="c.id">
          <StatusIcon :status="c.status" />
          <span>
            <template v-if="label(c.id)">{{ label(c.id) }}<template v-if="c.status === 'ok' && !settings.technical">: {{ $t("status.ok") }}</template></template>
            <span v-if="c.status !== 'ok' || settings.technical || !label(c.id)" :class="{ muted: !!label(c.id) }">
              <template v-if="label(c.id)"> — </template>{{ c.message }}
            </span>
            <code v-if="settings.technical" class="small muted"> ({{ c.id }})</code>
          </span>
        </li>
      </ul>
    </div>
  </div>
</template>

<style scoped>
.block + .block { margin-top: 0.75rem; }
.head { display: flex; gap: 0.75rem; align-items: baseline; flex-wrap: wrap; }
.checks { list-style: none; padding: 0; margin: 0.4rem 0 0; }
.checks li { display: flex; gap: 0.5rem; align-items: flex-start; margin: 0.3rem 0; }
</style>
