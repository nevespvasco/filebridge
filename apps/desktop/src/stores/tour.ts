// The guide on screen, if any (docs/design/specs/D02, "Guided tour"). One at a time; finishing or
// skipping marks its version as seen, so it starts by itself only once.
import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import { TOURS, tourTarget, type TourId, type TourStep } from "@/lib/tour";
import { useConnection } from "./connection";
import { useSettings } from "./settings";

export const useTour = defineStore("tour", () => {
  const settings = useSettings();
  const active = ref<TourId | null>(null);
  /** The steps of `active` whose element is on screen (or that don't need one). */
  const steps = shallowRef<TourStep[]>([]);
  const index = ref(0);
  const step = computed(() => steps.value[index.value] ?? null);
  const isLast = computed(() => index.value === steps.value.length - 1);

  /** Starts `id` unless guides are off, it was already seen, the service is away or one is open. */
  function maybeStart(id: TourId) {
    if (!settings.guides.auto || (settings.guides.seen[id] ?? 0) >= TOURS[id].version) return;
    if (useConnection().connected !== true || active.value) return;
    start(id);
  }

  /** Always starts (the "?" button). */
  function start(id: TourId) {
    const list = TOURS[id].steps.filter((s) => !s.optional || tourTarget(s));
    if (!list.length) return;
    steps.value = list;
    index.value = 0;
    active.value = id;
  }

  function next() {
    if (!active.value) return;
    if (isLast.value) finish();
    else index.value++;
  }
  function back() {
    if (index.value > 0) index.value--;
  }
  /** Skip and Done both mean "seen". */
  function finish() {
    if (!active.value) return;
    settings.markTourSeen(active.value, TOURS[active.value].version);
    active.value = null;
    steps.value = [];
  }
  /** The page under it went away. */
  function stop(id?: TourId) {
    if (active.value && (!id || id === active.value)) finish();
  }

  return { active, steps, index, step, isLast, maybeStart, start, next, back, skip: finish, finish, stop };
});
