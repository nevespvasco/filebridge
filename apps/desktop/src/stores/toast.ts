// Short confirmations of small actions ("Copied", "Saved"), in one place at the bottom of the
// window instead of next to a button that may be out of sight (docs/design/01-design-system.md,
// "Toast"). One at a time; a newer one replaces it.
import { defineStore } from "pinia";
import { shallowRef } from "vue";

export type Toast = { id: number; text: string; tone: "ok" | "error" };

export const useToast = defineStore("toast", () => {
  const current = shallowRef<Toast | null>(null);
  let seq = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  function show(text: string, tone: Toast["tone"] = "ok", ms = 2500) {
    clearTimeout(timer);
    current.value = { id: ++seq, text, tone };
    timer = setTimeout(() => (current.value = null), ms);
  }

  function dismiss() {
    clearTimeout(timer);
    current.value = null;
  }

  return { current, show, dismiss };
});
