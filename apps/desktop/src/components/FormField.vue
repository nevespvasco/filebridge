<script setup lang="ts">
// One form field (docs/design/01-design-system.md, "Forms"): a visible label tied to its control
// (`<label for>`, so a screen reader announces "Name", not the placeholder or the help), an
// optional "?" beside it, and a short hint under the control that describes it.
// The control comes from the slot, which receives the `id` and `describedby` to put on it.
import { useId } from "vue";
import HelpTip from "./HelpTip.vue";

defineProps<{ label: string; help?: string; example?: string; hint?: string; hintError?: boolean; required?: boolean }>();
const id = useId();
const hintId = `${id}-hint`;
</script>

<template>
  <div class="field">
    <span class="label-line">
      <label :for="id">{{ label }}</label><span v-if="required" class="req" aria-hidden="true">*</span>
      <HelpTip v-if="help" :text="help" :example="example" :label="label" />
    </span>
    <slot :id="id" :describedby="hint ? hintId : undefined" />
    <span v-if="hint" :id="hintId" class="hint" :class="{ error: hintError }"><slot name="hint">{{ hint }}</slot></span>
  </div>
</template>
