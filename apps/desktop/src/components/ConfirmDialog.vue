<script setup lang="ts">
// Every destructive action asks first and says what will happen (CLAUDE.md, docs/06).
import { nextTick, ref, watch } from "vue";

const props = defineProps<{
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
const cancelBtn = ref<HTMLButtonElement | null>(null);

watch(
  () => props.open,
  async (open) => {
    if (open) {
      await nextTick();
      cancelBtn.value?.focus();
    }
  },
);
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="overlay" @keydown.esc="emit('cancel')" @click.self="emit('cancel')">
      <div class="dialog card" role="alertdialog" aria-modal="true" :aria-label="title">
        <h2>{{ title }}</h2>
        <ul>
          <li v-for="c in consequences" :key="c">{{ c }}</li>
        </ul>
        <slot />
        <div class="actions">
          <button ref="cancelBtn" class="btn" :disabled="busy" @click="emit('cancel')">{{ cancelLabel ?? $t("common.cancel") }}</button>
          <button class="btn" :class="danger ? 'btn-danger' : 'btn-primary'" :disabled="busy" @click="emit('confirm')">
            {{ confirmLabel }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.overlay {
  position: fixed;
  inset: 0;
  background: rgb(0 0 0 / 0.45);
  display: grid;
  place-items: center;
  z-index: 50;
  padding: 1rem;
}
.dialog {
  max-width: 480px;
  width: 100%;
}
h2 { margin-top: 0; font-size: 1.15rem; }
ul { padding-left: 1.2rem; color: var(--text-2); }
li { margin: 0.35rem 0; }
.actions {
  display: flex;
  justify-content: flex-end;
  gap: 0.5rem;
  margin-top: 1rem;
}
</style>
