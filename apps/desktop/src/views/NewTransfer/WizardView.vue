<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useWizard, type Step } from "@/stores/wizard";
import StepSource from "./StepSource.vue";
import StepDestination from "./StepDestination.vue";
import StepAnalysis from "./StepAnalysis.vue";
import StepReview from "./StepReview.vue";
import StepAfter from "./StepAfter.vue";

const wizard = useWizard();
const { t } = useI18n();

const steps = computed(() =>
  ([1, 2, 3, 4, 5] as Step[]).map((n) => ({ n, label: t(`wizard.steps.${n}`) })),
);

/** Earlier steps can be revisited until the job is sent; after that only step 5 remains. */
function reachable(n: Step) {
  if (wizard.step === 5) return n === 5;
  if (n === 5) return false;
  if (n === 1) return true;
  if (n === 2) return wizard.sources.length > 0;
  if (n === 3) return !!wizard.jobId;
  return wizard.canReview;
}

function go(n: Step) {
  if (reachable(n)) wizard.step = n;
}
</script>

<template>
  <div class="wizard">
    <header class="page-head">
      <h1>{{ $t("wizard.title") }}</h1>
    </header>
    <ol class="stepper">
      <li v-for="s in steps" :key="s.n" :class="{ current: wizard.step === s.n, done: s.n < wizard.step }">
        <button :disabled="!reachable(s.n)" :aria-current="wizard.step === s.n ? 'step' : undefined" @click="go(s.n)">
          <span class="num">{{ s.n < wizard.step ? "✓" : s.n }}</span>
          <span class="label">{{ s.label }}</span>
        </button>
      </li>
    </ol>
    <StepSource v-if="wizard.step === 1" />
    <StepDestination v-else-if="wizard.step === 2" />
    <StepAnalysis v-else-if="wizard.step === 3" />
    <StepReview v-else-if="wizard.step === 4" />
    <StepAfter v-else />
  </div>
</template>

<style scoped>
.stepper { list-style: none; display: flex; gap: 0.25rem; padding: 0; margin: 0 0 1.5rem; flex-wrap: wrap; }
.stepper li { flex: 1; min-width: 120px; }
.stepper button {
  all: unset;
  box-sizing: border-box;
  display: flex;
  gap: 0.5rem;
  align-items: center;
  width: 100%;
  padding: 0.5rem 0.6rem;
  border-bottom: 3px solid var(--border);
  cursor: pointer;
  color: var(--text-2);
}
.stepper button:disabled { cursor: default; opacity: 0.6; }
.stepper button:focus-visible { outline: 2px solid var(--accent); }
.stepper .current button { border-color: var(--accent); color: var(--text); font-weight: 600; }
.stepper .done button { border-color: var(--ok); }
.num { display: inline-grid; place-items: center; width: 1.6rem; height: 1.6rem; border-radius: 50%; background: var(--surface-2); font-size: 0.85rem; flex: none; }
.current .num { background: var(--accent); color: var(--on-accent); }
.done .num { background: var(--ok); color: var(--on-accent); }
.label { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
</style>
