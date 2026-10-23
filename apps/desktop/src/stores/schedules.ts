// Scheduled sends (docs/adr/0019) and watched folders (docs/adr/0023). The service runs them at
// their times or when a watched folder changes; the window only lists, saves, removes and runs
// them now. The list is reloaded when a schedule's time comes (`schedule_ran`), when a watched
// folder changes (`schedule_changed`, at most a few times a second) and when the connection comes
// back.
import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { Recurrence, ScheduleInput_Deserialize as ScheduleInput, ScheduleView_Serialize as ScheduleView, Weekday } from "@/bindings";
import { api, errorMessage } from "@/lib/api";
import { useConnection } from "@/stores/connection";

export type { ScheduleInput, ScheduleView };

/** Monday first, as `Weekday` counts. */
export const WEEKDAYS: Weekday[] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/** `HH:MM` from minutes past midnight. */
export function hm(minute: number): string {
  return `${String(Math.floor(minute / 60)).padStart(2, "0")}:${String(minute % 60).padStart(2, "0")}`;
}

/** Minutes past midnight from `HH:MM`; `null` when it isn't a time. */
export function minuteOf(time: string): number | null {
  const m = /^(\d{1,2}):(\d{2})$/.exec(time.trim());
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2]);
  return h < 24 && min < 60 ? h * 60 + min : null;
}

/** Every day, Monday to Friday, or the days listed (in Monday-first order). */
export function dayGroup(days: Weekday[]): "daily" | "weekdays" | "some" {
  if (days.length === 7) return "daily";
  const work = WEEKDAYS.slice(0, 5);
  if (days.length === 5 && work.every((d) => days.includes(d))) return "weekdays";
  return "some";
}

export function sortedDays(days: Weekday[]): Weekday[] {
  return WEEKDAYS.filter((d) => days.includes(d));
}


export type { Recurrence };

export const useSchedules = defineStore("schedules", () => {
  const items = ref<ScheduleView[]>([]);
  const loaded = ref(false);
  const loadError = ref<string | null>(null);
  const byId = computed(() => Object.fromEntries(items.value.map((s) => [s.id, s])));
  let watching = false;

  async function load() {
    try {
      items.value = await api.listSchedules();
      loadError.value = null;
    } catch (e) {
      loadError.value = errorMessage(e);
    } finally {
      loaded.value = true;
    }
    if (!watching) {
      watching = true;
      useConnection().onReconnect(() => void load());
    }
  }

  function put(s: ScheduleView) {
    const i = items.value.findIndex((x) => x.id === s.id);
    if (i >= 0) items.value.splice(i, 1, s);
    else items.value = [...items.value, s].sort((a, b) => a.name.localeCompare(b.name));
  }

  async function save(input: ScheduleInput): Promise<ScheduleView> {
    const s = await api.saveSchedule(input);
    put(s);
    return s;
  }

  async function remove(id: string) {
    await api.deleteSchedule(id);
    items.value = items.value.filter((s) => s.id !== id);
  }

  /** The run's job, or `null` when nothing changed since the last run. */
  async function runNow(id: string): Promise<string | null> {
    try {
      return await api.runScheduleNow(id);
    } finally {
      await load();
    }
  }

  /** A schedule's time came or its folder changed: its row changed. Reloads at most once per
   * half second, so a burst of changes doesn't flood the service. */
  let reload: ReturnType<typeof setTimeout> | null = null;
  function onRan() {
    if (!loaded.value || reload) return;
    reload = setTimeout(() => {
      reload = null;
      void load();
    }, 500);
  }

  return { items, loaded, loadError, byId, load, save, remove, runNow, onRan };
});
