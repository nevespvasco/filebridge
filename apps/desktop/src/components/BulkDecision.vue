<script setup lang="ts">
// "6 names need a decision": one button accepts every suggested fix that may be accepted without a
// look, the other opens the groups to decide one by one (docs/04, "Decisions model"). Garbled
// names (mojibake) and names with no proposal are left out of the button and say why: they are
// never fixed without the user reading them.
import { computed, inject, ref } from "vue";
import type { RuleCount } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { errorMessage } from "@/lib/api";
import { DECISIONS, type Decisions, bulkAcceptable } from "@/lib/decisions";
import { useWizard } from "@/stores/wizard";
import AppIcon from "./AppIcon.vue";

const props = defineProps<{ rules: RuleCount[] }>();
const emit = defineEmits<{ review: [] }>();
const fmt = useFmt();
const decisions: Decisions = inject(DECISIONS, null) ?? useWizard();

const pending = computed(() => props.rules.reduce((n, r) => n + r.pending, 0));
const acceptable = computed(() => props.rules.filter((r) => r.pending > 0 && bulkAcceptable(r)));
const n = computed(() => acceptable.value.reduce((sum, r) => sum + r.pending, 0));
const left = computed(() => pending.value - n.value);
const busy = ref(false);
const error = ref<string | null>(null);

async function acceptAll() {
  busy.value = true;
  error.value = null;
  try {
    for (const r of acceptable.value) await decisions.decideAll(r.rule, { kind: "accept" });
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="guide warn bulk-decision" role="status">
    <AppIcon class="guide-icon" name="hand" :size="20" />
    <div class="body">
      <p><strong>{{ $t("analysis.guidePendingTitle", { n: fmt.count(pending) }, pending) }}</strong></p>
      <p class="small">{{ n ? $t("analysis.guidePending") : $t("analysis.guideLook") }}</p>
      <p v-if="n && left" class="small">{{ $t("analysis.notBulk", { n: fmt.count(left) }, left) }}</p>
      <div class="buttons">
        <button v-if="n" class="btn btn-primary" :disabled="busy" @click="acceptAll">
          <AppIcon name="check" /> {{ busy ? $t("issues.bulkProgress") : $t("analysis.acceptSuggestions", { n: fmt.count(n) }, n) }}
        </button>
        <button class="btn" :disabled="busy" @click="emit('review')">{{ $t("analysis.reviewEach") }}</button>
      </div>
      <p v-if="error" class="error small" role="alert">{{ error }}</p>
    </div>
  </div>
</template>

<style scoped>
.body { display: grid; gap: var(--sp-2); min-width: 0; }
.buttons { display: flex; gap: var(--sp-2); flex-wrap: wrap; margin-top: var(--sp-1); }
</style>
