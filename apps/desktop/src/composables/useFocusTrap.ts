// Focus for anything modal (docs/design/01-design-system.md, "Dialogs"): when it opens, focus moves
// in (to `[data-autofocus]`, else the first control, else the panel) and Tab stays inside; Esc
// closes it unless it is busy; when it closes, focus goes back where it was.
import { nextTick, onBeforeUnmount, watch, type Ref } from "vue";

const FOCUSABLE = 'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

export function focusables(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter(
    (el) => !el.hasAttribute("disabled") && el.getAttribute("aria-hidden") !== "true" && el.offsetParent !== null,
  );
}

export function useFocusTrap(
  panel: Ref<HTMLElement | null>,
  open: () => boolean,
  close: () => void,
  locked: () => boolean = () => false,
) {
  let returnTo: HTMLElement | null = null;

  watch(
    open,
    async (isOpen) => {
      if (isOpen) {
        returnTo = document.activeElement as HTMLElement | null;
        await nextTick();
        const root = panel.value;
        if (!root) return;
        const first = root.querySelector<HTMLElement>("[data-autofocus]") ?? focusables(root)[0] ?? root;
        first.focus({ preventScroll: false });
      } else {
        restore();
      }
    },
    { immediate: true },
  );

  function restore() {
    const el = returnTo;
    returnTo = null;
    if (el && document.contains(el)) el.focus?.();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      if (!locked()) close();
      return;
    }
    if (e.key !== "Tab" || !panel.value) return;
    const items = focusables(panel.value);
    if (!items.length) {
      e.preventDefault();
      return;
    }
    const [first, last] = [items[0], items[items.length - 1]];
    const at = document.activeElement;
    if (e.shiftKey && (at === first || at === panel.value)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && at === last) {
      e.preventDefault();
      first.focus();
    }
  }

  onBeforeUnmount(restore);
  return { onKey };
}
