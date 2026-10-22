<script setup lang="ts">
// A small "?" next to a label or term: hovering, focusing or clicking it shows a plain-language
// explanation, optionally with an example. The text is always in the DOM for screen readers
// (`aria-describedby`); the bubble itself is placed in `body` so cards never clip it.
// With a default slot, the slot is the trigger instead of the "?" (and names the button).
import { nextTick, onBeforeUnmount, ref, useId, useSlots } from "vue";

const props = defineProps<{ text: string; example?: string; label?: string }>();
const custom = !!useSlots().default;
const id = useId();
const btn = ref<HTMLButtonElement | null>(null);
const bubble = ref<HTMLDivElement | null>(null);
const open = ref(false);
/** Opened by a click: stays until the next click, Esc or a click elsewhere. */
const pinned = ref(false);
const pos = ref({ top: 0, left: 0, below: false });

async function place() {
  open.value = true;
  await nextTick();
  const b = btn.value?.getBoundingClientRect();
  const w = bubble.value?.offsetWidth ?? 280;
  const h = bubble.value?.offsetHeight ?? 60;
  if (!b) return;
  const margin = 8;
  const left = Math.min(Math.max(margin, b.left + b.width / 2 - w / 2), window.innerWidth - w - margin);
  const below = b.top - h - margin < 0;
  pos.value = { left, top: below ? b.bottom + margin : b.top - h - margin, below };
}

function show() {
  if (!open.value) void place();
}
function hide() {
  if (!pinned.value) open.value = false;
}
function toggle() {
  pinned.value = !pinned.value;
  if (pinned.value) void place();
  else open.value = false;
}
function close() {
  pinned.value = false;
  open.value = false;
}
function onDocClick(e: MouseEvent) {
  if (pinned.value && e.target instanceof Node && !btn.value?.contains(e.target)) close();
}
document.addEventListener("click", onDocClick);
window.addEventListener("scroll", close, true);
onBeforeUnmount(() => {
  document.removeEventListener("click", onDocClick);
  window.removeEventListener("scroll", close, true);
});
</script>

<template>
  <span class="help-tip" :class="{ custom }">
    <button
      ref="btn"
      type="button"
      :class="custom ? 'trigger' : 'q'"
      :aria-label="custom ? undefined : props.label ? $t('help.about', { what: props.label }) : $t('help.more')"
      :aria-describedby="id"
      :aria-expanded="open"
      @mouseenter="show"
      @mouseleave="hide"
      @focus="show"
      @blur="hide"
      @click.stop.prevent="toggle"
      @keydown.esc="close"
    ><slot>?</slot></button>
    <span :id="id" class="sr-only">{{ text }}<template v-if="example"> {{ $t("help.example") }} {{ example }}</template></span>
    <Teleport to="body">
      <div
        v-if="open"
        ref="bubble"
        class="help-bubble"
        :class="{ below: pos.below }"
        :style="{ top: `${pos.top}px`, left: `${pos.left}px` }"
        aria-hidden="true"
      >
        <span>{{ text }}</span>
        <span v-if="example" class="ex"><strong>{{ $t("help.example") }}</strong> {{ example }}</span>
      </div>
    </Teleport>
  </span>
</template>

<style scoped>
.help-tip { display: inline-flex; vertical-align: middle; margin-left: 0.3rem; }
.help-tip.custom { margin-left: 0; }
.trigger { all: unset; box-sizing: border-box; display: inline-flex; align-items: center; gap: 0.4rem; cursor: help; border-radius: var(--radius-sm); }
.trigger:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.q {
  all: unset;
  box-sizing: border-box;
  display: inline-grid;
  place-items: center;
  width: 1.15rem;
  height: 1.15rem;
  border-radius: 50%;
  border: 1px solid var(--border-strong);
  color: var(--text-2);
  font-size: 0.72rem;
  font-weight: 700;
  line-height: 1;
  cursor: help;
  background: var(--surface);
}
.q:hover, .q[aria-expanded="true"] { border-color: var(--accent); color: var(--accent); }
.q:focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
</style>

<style>
/* Unscoped: the bubble is teleported to <body>. */
.help-bubble {
  position: fixed;
  z-index: 100;
  max-width: min(300px, calc(100vw - 16px));
  display: grid;
  gap: 0.35rem;
  padding: 0.55rem 0.7rem;
  border-radius: 8px;
  background: var(--text);
  color: var(--bg);
  font-size: 0.82rem;
  font-weight: 400;
  line-height: 1.4;
  box-shadow: 0 6px 20px rgb(0 0 0 / 0.25);
  pointer-events: none;
  text-align: left;
  white-space: normal;
}
.help-bubble .ex { opacity: 0.85; }
</style>
