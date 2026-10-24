// One question at a time for flows that aren't a page's own action (CLAUDE.md: what will happen,
// before it happens):
// `await useConfirm().ask({...})` shows the app's confirmation dialog and resolves with the answer.
import { defineStore } from "pinia";
import { shallowRef } from "vue";

export type ConfirmRequest = {
  title: string;
  consequences: string[];
  confirmLabel: string;
  cancelLabel?: string;
  danger?: boolean;
};

export const useConfirm = defineStore("confirm", () => {
  const request = shallowRef<ConfirmRequest | null>(null);
  let settle: ((yes: boolean) => void) | null = null;

  /** A newer question answers the older one with "no". */
  function ask(r: ConfirmRequest): Promise<boolean> {
    settle?.(false);
    request.value = r;
    return new Promise((resolve) => (settle = resolve));
  }

  function answer(yes: boolean) {
    request.value = null;
    settle?.(yes);
    settle = null;
  }

  return { request, ask, answer };
});
