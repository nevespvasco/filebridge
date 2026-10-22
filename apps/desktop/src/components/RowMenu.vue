<script setup lang="ts">
// The ⋮ menu of a row (docs/design/01-design-system.md, "Components"): arrow keys move, Esc
// closes and gives focus back, a disabled item says why.
import { nextTick, onBeforeUnmount, ref } from "vue";
import type { IconName } from "@/lib/icons";
import AppIcon from "./AppIcon.vue";

export type MenuItem = { id: string; label: string; icon?: IconName; danger?: boolean; disabled?: boolean; reason?: string };

defineProps<{ items: MenuItem[]; label: string }>();
const emit = defineEmits<{ select: [id: string] }>();
const open = ref(false);
/** Fixed position under the trigger: the menu lives in <body>, so no scroll box clips it. */
const pos = ref<{ top: string; right?: string; bottom?: string; maxHeight?: string }>({ top: "0px" });
const trigger = ref<HTMLButtonElement | null>(null);
const menu = ref<HTMLElement | null>(null);

function place() {
  const r = trigger.value?.getBoundingClientRect();
  if (!r) return;
  // Toward the side with more room, never taller than that room (it scrolls instead), so a
  // short or narrow window never cuts it off.
  const room = { below: window.innerHeight - r.bottom - 12, above: r.top - 12 };
  const below = room.below > 260 || room.below >= room.above;
  const right = `${Math.max(8, window.innerWidth - r.right)}px`;
  pos.value = below
    ? { top: `${r.bottom + 4}px`, right, maxHeight: `${room.below}px` }
    : { top: "auto", bottom: `${window.innerHeight - r.top + 4}px`, right, maxHeight: `${room.above}px` };
}
async function show() {
  place();
  open.value = true;
  window.addEventListener("scroll", closeQuietly, true);
  window.addEventListener("resize", closeQuietly);
  await nextTick();
  menu.value?.querySelector<HTMLElement>("[role=menuitem]:not([aria-disabled=true])")?.focus();
  document.addEventListener("pointerdown", outside, true);
}
function closeQuietly() {
  hide(false);
}
function hide(focusBack = true) {
  open.value = false;
  document.removeEventListener("pointerdown", outside, true);
  window.removeEventListener("scroll", closeQuietly, true);
  window.removeEventListener("resize", closeQuietly);
  if (focusBack) trigger.value?.focus();
}
function outside(e: Event) {
  if (!menu.value?.contains(e.target as Node) && e.target !== trigger.value) hide(false);
}
function onKey(e: KeyboardEvent) {
  const items = [...(menu.value?.querySelectorAll<HTMLElement>("[role=menuitem]") ?? [])];
  const i = items.indexOf(document.activeElement as HTMLElement);
  if (e.key === "Escape") hide();
  else if (e.key === "ArrowDown") items[(i + 1) % items.length]?.focus();
  else if (e.key === "ArrowUp") items[(i - 1 + items.length) % items.length]?.focus();
  else if (e.key === "Tab") hide(false);
  else return;
  e.preventDefault();
}
function pick(item: MenuItem) {
  if (item.disabled) return;
  hide();
  emit("select", item.id);
}
onBeforeUnmount(() => {
  document.removeEventListener("pointerdown", outside, true);
  window.removeEventListener("scroll", closeQuietly, true);
  window.removeEventListener("resize", closeQuietly);
});
</script>

<template>
  <div class="row-menu" @click.stop>
    <button ref="trigger" type="button" class="btn btn-small trigger" :aria-label="label" aria-haspopup="menu" :aria-expanded="open" @click="open ? hide() : show()">
      <AppIcon name="menu" />
    </button>
    <Teleport to="body">
    <div v-if="open" ref="menu" class="menu" role="menu" :aria-label="label" :style="pos" @keydown="onKey" @click.stop>
      <button
        v-for="it in items"
        :key="it.id"
        type="button"
        class="item"
        :class="{ danger: it.danger }"
        role="menuitem"
        :aria-disabled="it.disabled ? 'true' : undefined"
        :title="it.disabled ? it.reason : undefined"
        tabindex="-1"
        @click="pick(it)"
      >
        <AppIcon v-if="it.icon" :name="it.icon" />
        <span>{{ it.label }}</span>
      </button>
    </div>
    </Teleport>
  </div>
</template>

<style scoped>
.row-menu { position: relative; display: inline-block; }
.trigger { padding: 0.25rem 0.4rem; }
.menu { position: fixed; z-index: 60; min-width: min(15rem, calc(100vw - 16px)); max-width: calc(100vw - 16px); overflow-y: auto; padding: 0.3rem; background: var(--surface); border: 1px solid var(--border); border-radius: var(--radius-md); box-shadow: var(--shadow-pop); display: grid; }
.item { font: inherit; display: flex; gap: 0.55rem; align-items: center; text-align: left; padding: 0.45rem 0.6rem; border: 0; background: none; color: var(--text); border-radius: var(--radius-sm); cursor: pointer; }
.item:hover, .item:focus-visible { background: var(--surface-2); outline: none; }
.item.danger { color: var(--danger); }
.item[aria-disabled="true"] { color: var(--muted); cursor: not-allowed; }
</style>
