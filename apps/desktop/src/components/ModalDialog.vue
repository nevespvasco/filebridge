<script setup lang="ts">
// The one dialog (docs/design/01-design-system.md, "Dialogs"): a card over a dimmed page, named
// by its title, with focus moved in and kept in, Esc and the backdrop to close (unless busy), and
// focus back on the control that opened it. Every dialog of the app is built on it.
import { ref, useId } from "vue";
import { useFocusTrap } from "@/composables/useFocusTrap";

const props = withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    /** `alertdialog` for a confirmation that interrupts (destructive actions). */
    role?: "dialog" | "alertdialog";
    width?: number;
    /** While true, Esc and the backdrop don't close it (an action is running). */
    busy?: boolean;
  }>(),
  { role: "dialog", width: 480, busy: false },
);
const emit = defineEmits<{ close: [] }>();
const panel = ref<HTMLElement | null>(null);
const titleId = useId();
const { onKey } = useFocusTrap(
  panel,
  () => props.open,
  () => emit("close"),
  () => props.busy,
);
</script>

<template>
  <Teleport to="body">
    <div v-if="open" class="modal-overlay" @click.self="!busy && emit('close')">
      <div
        ref="panel"
        class="modal card"
        :role="role"
        aria-modal="true"
        :aria-labelledby="titleId"
        tabindex="-1"
        :style="{ width: `min(${width}px, 100%)` }"
        @keydown="onKey"
      >
        <h2 :id="titleId" class="modal-title"><slot name="title">{{ title }}</slot></h2>
        <slot />
        <div v-if="$slots.actions" class="modal-actions"><slot name="actions" /></div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.modal-overlay { position: fixed; inset: 0; background: var(--overlay); display: grid; place-items: center; z-index: 50; padding: var(--sp-4); }
.modal { display: grid; gap: var(--sp-3); max-height: calc(100vh - 2rem); overflow: auto; outline: none; border-radius: var(--radius-lg); box-shadow: var(--shadow-pop); }
.modal-title { margin: 0; font-size: var(--fs-lg); overflow-wrap: anywhere; }
.modal-actions { display: flex; justify-content: flex-end; gap: var(--sp-2); flex-wrap: wrap; margin-top: var(--sp-1); }
</style>
