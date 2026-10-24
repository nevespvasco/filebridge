<script setup lang="ts">
// Transport and integration preflight checks (free space, write access, tool checks), in plain
// words. Each thing is said once: a block whose checks all pass is one line ("✓ Integration ·
// can write", its checks one tap away); a check that isn't OK shows with its own message, cut
// after a few lines with "More". Its code only with "Show technical information" on, which also
// shows every check.
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import type { PreflightView_Serialize } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { useSettings } from "@/stores/settings";
import AppIcon from "./AppIcon.vue";
import ClampText from "./ClampText.vue";
import StatusIcon from "./StatusIcon.vue";

defineProps<{ views: PreflightView_Serialize[] }>();
const fmt = useFmt();
const settings = useSettings();
const { t, te } = useI18n();
const label = (id: string) => (te(`preflight.check.${id}`) ? t(`preflight.check.${id}`) : null);

type Check = PreflightView_Serialize["checks"][number];
const problems = (v: PreflightView_Serialize) => v.checks.filter((c) => c.status !== "ok");
const passed = (v: PreflightView_Serialize) => v.checks.filter((c) => c.status === "ok");
/** The OK checks a block shows: all with technical information on, else those opened. */
const open = ref<Record<string, boolean>>({});
const shown = (v: PreflightView_Serialize): Check[] =>
  settings.technical ? v.checks : [...problems(v), ...(open.value[v.source] ? passed(v) : [])];
</script>

<template>
  <div class="preflight">
    <div v-for="v in views" :key="v.source" class="block">
      <div class="head">
        <StatusIcon v-if="!problems(v).length" status="ok" />
        <strong>{{ $t(`preflight.${v.source}`) }}</strong>
        <span class="muted small">
          <template v-if="v.free_bytes != null">{{ $t("preflight.free", { size: fmt.bytes(v.free_bytes) }) }} · </template>{{ v.writable ? $t("preflight.writable") : $t("preflight.notWritable") }}
        </span>
        <button
          v-if="!settings.technical && passed(v).length"
          class="link-btn toggle"
          type="button"
          :aria-expanded="!!open[v.source]"
          @click="open[v.source] = !open[v.source]"
        >
          <AppIcon :name="open[v.source] ? 'caretDown' : 'caretRight'" :size="14" />
          {{ $t("preflight.passed", { n: passed(v).length }, passed(v).length) }}
        </button>
      </div>
      <ul v-if="shown(v).length" class="checks">
        <li v-for="c in shown(v)" :key="c.id">
          <StatusIcon :status="c.status" />
          <span class="what">
            <template v-if="label(c.id)">
              <span>{{ label(c.id) }}<template v-if="c.status === 'ok' && !settings.technical">: {{ $t("status.ok") }}</template></span>
            </template>
            <ClampText
              v-if="c.status !== 'ok' || settings.technical || !label(c.id)"
              :class="{ muted: !!label(c.id) }"
              :text="c.message"
              :lines="3"
            />
            <code v-if="settings.technical" class="small muted">({{ c.id }})</code>
          </span>
        </li>
      </ul>
    </div>
  </div>
</template>

<style scoped>
.block + .block { margin-top: 0.75rem; }
.head { display: flex; gap: 0.3rem 0.6rem; align-items: center; flex-wrap: wrap; }
.toggle { margin-left: auto; }
.checks { list-style: none; padding: 0; margin: 0.4rem 0 0; }
.checks li { display: flex; gap: 0.5rem; align-items: flex-start; margin: 0.3rem 0; }
.what { display: grid; gap: 0.1rem; min-width: 0; }
</style>
