<script setup lang="ts">
// Every destructive action asks first and says what will happen (CLAUDE.md, docs/06). Focus
// starts on the safe button.
import ModalDialog from "./ModalDialog.vue";

defineProps<{
  open: boolean;
  title: string;
  consequences: string[];
  confirmLabel: string;
  /** Defaults to "Cancel"; use something clearer when the action itself is a cancel. */
  cancelLabel?: string;
  danger?: boolean;
  busy?: boolean;
}>();
const emit = defineEmits<{ confirm: []; cancel: [] }>();
</script>

<template>
  <ModalDialog :open="open" :title="title" :role="danger ? 'alertdialog' : 'dialog'" :busy="busy" @close="emit('cancel')">
    <ul v-if="consequences.length">
      <li v-for="c in consequences" :key="c">{{ c }}</li>
    </ul>
    <slot />
    <template #actions>
      <button data-autofocus class="btn" :disabled="busy" @click="emit('cancel')">{{ cancelLabel ?? $t("common.cancel") }}</button>
      <button class="btn" :class="danger ? 'btn-danger' : 'btn-primary'" :disabled="busy" @click="emit('confirm')">
        {{ confirmLabel }}
      </button>
    </template>
  </ModalDialog>
</template>

<style scoped>
ul { padding-left: 1.2rem; margin: 0; color: var(--text-2); }
li { margin: var(--sp-1) 0; }
</style>
