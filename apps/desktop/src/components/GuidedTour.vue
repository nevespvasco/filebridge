<script setup lang="ts">
// The guide on screen (docs/design/specs/D02, "Guided tour"): a spotlight on one element and a
// card next to it with Back / Next / Skip. While it is open the page under it is inert; Esc skips,
// ← and → move between steps, and focus comes back where it was when it closes.
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { NARROW, place, tourTarget, type Rect } from "@/lib/tour";
import { useSettings } from "@/stores/settings";
import { useTour } from "@/stores/tour";

const tour = useTour();
const settings = useSettings();
const route = useRoute();

const pop = ref<HTMLElement | null>(null);
const nextBtn = ref<HTMLButtonElement | null>(null);
const spot = ref<Rect | null>(null);
const pos = ref({ top: 0, left: 0 });
const narrow = ref(false);
const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
let returnFocus: HTMLElement | null = null;
let observer: ResizeObserver | null = null;
let frame = 0;

const PAD = 6;

function measure() {
  const step = tour.step;
  if (!step || !pop.value) return;
  const el = tourTarget(step);
  const r = el?.getBoundingClientRect();
  spot.value = r ? { top: r.top - PAD, left: r.left - PAD, width: r.width + PAD * 2, height: r.height + PAD * 2 } : null;
  // Below 760px the tab bar is fixed at the bottom (App.vue): the popover stays above it.
  const places = document.querySelector<HTMLElement>(".places");
  const bar = places && getComputedStyle(places).position === "fixed" ? places.getBoundingClientRect().top : 0;
  const view = { width: window.innerWidth, height: bar > 0 ? bar : window.innerHeight };
  narrow.value = view.width < NARROW;
  const size = pop.value.getBoundingClientRect();
  const p = place(spot.value, { width: size.width, height: size.height }, view, step.placement);
  pos.value = { top: p.top, left: p.left };
}
function schedule() {
  cancelAnimationFrame(frame);
  frame = requestAnimationFrame(measure);
}

async function show() {
  const step = tour.step;
  if (!step) return;
  await nextTick();
  const el = tourTarget(step);
  observer?.disconnect();
  observer = new ResizeObserver(schedule);
  if (el) {
    const r = el.getBoundingClientRect();
    if (r.top < 0 || r.bottom > window.innerHeight) {
      el.scrollIntoView({ block: "center", behavior: reduced.matches ? "auto" : "smooth" });
    }
    observer.observe(el);
  }
  if (pop.value) observer.observe(pop.value);
  measure();
  nextBtn.value?.focus({ preventScroll: true });
}

function open() {
  returnFocus = document.activeElement as HTMLElement | null;
  document.getElementById("app")?.setAttribute("inert", "");
  window.addEventListener("resize", schedule);
  window.addEventListener("scroll", schedule, true);
  document.addEventListener("keydown", onKey, true);
}
function close() {
  document.getElementById("app")?.removeAttribute("inert");
  window.removeEventListener("resize", schedule);
  window.removeEventListener("scroll", schedule, true);
  document.removeEventListener("keydown", onKey, true);
  observer?.disconnect();
  observer = null;
  cancelAnimationFrame(frame);
  spot.value = null;
  if (returnFocus?.isConnected) returnFocus.focus({ preventScroll: true });
  returnFocus = null;
}

watch(
  () => tour.active,
  (now, before) => {
    if (now && !before) open();
    else if (!now && before) close();
  },
);
watch(
  () => [tour.active, tour.index] as const,
  ([a]) => {
    if (a) void show();
  },
);
// The page under it went away.
watch(
  () => route.fullPath,
  () => tour.stop(),
);
onBeforeUnmount(() => {
  if (tour.active) close();
});

function onKey(e: KeyboardEvent) {
  if (!tour.active) return;
  if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    tour.skip();
  } else if (e.key === "ArrowRight" && !(e.target instanceof HTMLInputElement)) {
    e.preventDefault();
    tour.next();
  } else if (e.key === "ArrowLeft" && !(e.target instanceof HTMLInputElement)) {
    e.preventDefault();
    tour.back();
  } else if (e.key === "Tab" && pop.value) {
    // Focus stays in the card.
    const items = [...pop.value.querySelectorAll<HTMLElement>("button:not(:disabled), input")];
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    const inside = pop.value.contains(document.activeElement);
    if (e.shiftKey && (document.activeElement === first || !inside)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (document.activeElement === last || !inside)) {
      e.preventDefault();
      first.focus();
    }
  }
}

const autoOff = computed({
  get: () => !settings.guides.auto,
  set: (off: boolean) => (settings.guides = { ...settings.guides, auto: !off }),
});
const announce = computed(() =>
  tour.step ? `${tour.index + 1} / ${tour.steps.length}` : "",
);
</script>

<template>
  <Teleport to="body">
    <div v-if="tour.active && tour.step" class="tour" :class="{ narrow }">
      <!-- Clicks outside the card do nothing: the guide ends with Skip, Done or Esc. -->
      <div class="blocker" :class="{ dim: !spot }" />
      <div
        v-if="spot"
        class="spot"
        :style="{ top: `${spot.top}px`, left: `${spot.left}px`, width: `${spot.width}px`, height: `${spot.height}px` }"
        aria-hidden="true"
      />
      <div
        ref="pop"
        class="pop card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="tour-title"
        aria-describedby="tour-body"
        :style="{ top: `${pos.top}px`, left: `${pos.left}px` }"
      >
        <div class="head">
          <span class="count small muted num">{{ $t("tour.count", { n: tour.index + 1, total: tour.steps.length }) }}</span>
          <span class="dots" aria-hidden="true"><span v-for="(_, i) in tour.steps" :key="i" :class="{ on: i === tour.index }" /></span>
        </div>
        <h2 id="tour-title">{{ $t(tour.step.title, tour.step.params ?? {}) }}</h2>
        <p id="tour-body">{{ $t(tour.step.body, tour.step.params ?? {}) }}</p>
        <label class="auto small muted"><input v-model="autoOff" type="checkbox" /> {{ $t("tour.dontShow") }}</label>
        <div class="actions">
          <button class="btn btn-small skip" type="button" @click="tour.skip()">{{ $t("tour.skip") }}</button>
          <span class="grow" />
          <button v-if="tour.index > 0" class="btn btn-small" type="button" @click="tour.back()">{{ $t("common.back") }}</button>
          <button ref="nextBtn" class="btn btn-small btn-primary" type="button" @click="tour.next()">
            {{ tour.isLast ? $t("tour.done") : $t("common.next") }}
          </button>
        </div>
      </div>
      <p class="sr-only" aria-live="polite">{{ $t("tour.announce", { step: announce, title: $t(tour.step.title, tour.step.params ?? {}) }) }}</p>
    </div>
  </Teleport>
</template>

<style scoped>
.blocker { position: fixed; inset: 0; z-index: 80; }
.blocker.dim { background: var(--overlay); }
.spot {
  position: fixed;
  z-index: 81;
  border-radius: var(--radius-md);
  box-shadow: 0 0 0 9999px var(--overlay);
  outline: 2px solid var(--accent);
  outline-offset: 0;
  pointer-events: none;
  transition: top 0.2s ease, left 0.2s ease, width 0.2s ease, height 0.2s ease;
}
.pop {
  position: fixed;
  z-index: 82;
  width: min(340px, calc(100vw - 24px));
  box-shadow: var(--shadow-pop);
  display: grid;
  gap: 0.55rem;
  transition: top 0.2s ease, left 0.2s ease;
}
.narrow .pop { width: calc(100vw - 24px); }
.head { display: flex; align-items: center; gap: 0.6rem; }
.dots { display: inline-flex; gap: 0.3rem; }
.dots span { width: 6px; height: 6px; border-radius: 50%; background: var(--border-strong); }
.dots span.on { background: var(--accent); width: 16px; border-radius: var(--radius-xs); }
h2 { margin: 0; font-size: var(--fs-lg); }
p { margin: 0; color: var(--text-2); line-height: 1.45; }
.auto { display: flex; gap: 0.4rem; align-items: center; cursor: pointer; }
.actions { display: flex; gap: 0.4rem; align-items: center; }
.grow { flex: 1; }
.skip { background: transparent; border-color: transparent; color: var(--text-2); }
.skip:hover { color: var(--text); }
@media (prefers-reduced-motion: reduce) {
  .spot, .pop { transition: none; }
}
</style>
