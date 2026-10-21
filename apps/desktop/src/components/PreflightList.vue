<script setup lang="ts">
// Transport and integration preflight checks (free space, write access, tool checks).
import type { PreflightView_Serialize } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import StatusIcon from "./StatusIcon.vue";

defineProps<{ views: PreflightView_Serialize[] }>();
const fmt = useFmt();
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
          <span><code class="small">{{ c.id }}</code> {{ c.message }}</span>
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
