<script setup lang="ts">
// Checks a sealed report (ADR 0025): whether its JSON and its readable copy are exactly as
// exported. The service reads the files; the window only picks one.
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import type { ReportCheck_Serialize, SealProblem } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { api, errorMessage } from "@/lib/api";
import { copyText, pickPaths } from "@/lib/platform";
import AppIcon from "./AppIcon.vue";

defineProps<{ open: boolean }>();
const emit = defineEmits<{ close: [] }>();
const fmt = useFmt();
const { t } = useI18n();

const busy = ref(false);
const error = ref<string | null>(null);
const result = ref<ReportCheck_Serialize | null>(null);

async function choose() {
  error.value = null;
  const [path] = await pickPaths({ directory: false, multiple: false });
  if (!path) return;
  busy.value = true;
  result.value = null;
  try {
    result.value = await api.checkReport(path);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}

function close() {
  result.value = null;
  error.value = null;
  emit("close");
}

/** One problem, in words. */
function problem(p: SealProblem): string {
  switch (p.kind) {
    case "entry":
      return t("report.check.problem.entry", { n: fmt.count(p.index + 1), path: p.source_path });
    case "unreadable":
      return t("report.check.problem.unreadable", { detail: p.detail });
    default:
      return t(`report.check.problem.${p.kind}`);
  }
}
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="overlay" @click.self="close" @keydown.esc="close">
      <div class="dialog card" role="dialog" aria-modal="true" :aria-label="$t('report.check.title')">
        <h2>{{ $t("report.check.title") }}</h2>
        <p class="small muted">{{ $t("report.check.intro") }}</p>
        <p v-if="busy" class="muted" aria-live="polite">{{ $t("report.check.checking") }}</p>
        <p v-if="error" class="error small" role="alert">{{ error }}</p>
        <template v-if="result">
          <div v-if="result.intact" class="ok-text" role="status">
            <AppIcon name="checkCircle" /> {{ $t("report.check.intact", { n: fmt.count(result.entries) }, result.entries) }}
          </div>
          <div v-else class="error" role="status">
            <AppIcon name="failCircle" /> {{ $t("report.check.changed") }}
          </div>
          <p class="path small">{{ result.path }}<template v-if="result.readable"><br />{{ result.readable }}</template></p>
          <p v-if="result.intact && !result.readable" class="small muted">{{ $t("report.check.noCopy") }}</p>
          <ul v-if="result.problems.length" class="problems small">
            <li v-for="(p, i) in result.problems" :key="i">{{ problem(p) }}</li>
            <li v-if="result.more">{{ $t("report.check.more", { n: fmt.count(result.more) }) }}</li>
          </ul>
          <div v-if="result.sha256" class="seal">
            <span class="small muted">{{ $t("report.sealCode") }}</span>
            <span class="hash">{{ result.sha256 }}</span>
            <button class="btn btn-small" @click="copyText(result.sha256)"><AppIcon name="copy" /> {{ $t("report.copySeal") }}</button>
            <span v-if="result.intact" class="small muted">{{ $t("report.check.compare") }}</span>
          </div>
        </template>
        <div class="actions">
          <button class="btn" @click="close">{{ $t("common.close") }}</button>
          <button class="btn btn-primary" :disabled="busy" @click="choose"><AppIcon name="checksum" /> {{ $t("report.check.choose") }}</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.overlay { position: fixed; inset: 0; background: var(--overlay); display: grid; place-items: center; z-index: 50; padding: 1rem; }
.dialog { width: min(560px, 100%); display: grid; gap: 0.8rem; }
h2 { margin: 0; font-size: 1.1rem; }
p { margin: 0; }
.problems { margin: 0; padding-left: 1.2rem; display: grid; gap: 0.2rem; overflow-wrap: anywhere; }
.seal { display: grid; gap: 0.3rem; justify-items: start; }
.actions { display: flex; justify-content: flex-end; gap: 0.5rem; }
</style>
