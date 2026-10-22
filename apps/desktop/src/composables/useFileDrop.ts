// Files and folders dropped on the window, while the calling component is on screen: a
// kept-alive wizard in the background must not take drops meant for another page (Home has its
// own drop zone).
import { onActivated, onBeforeUnmount, onDeactivated, onMounted, ref } from "vue";
import { onFileDrop } from "@/lib/platform";

export function useFileDrop(onDrop: (paths: string[]) => void) {
  /** Something is being dragged over the window. */
  const over = ref(false);
  let unlisten: (() => void) | null = null;
  let active = false;

  async function listen() {
    active = true;
    if (unlisten) return;
    const off = await onFileDrop((state, paths) => {
      over.value = state === "over";
      if (state === "drop" && paths.length) onDrop(paths);
    });
    // Left the screen while subscribing: don't keep the listener.
    if (active && !unlisten) unlisten = off;
    else off();
  }
  function stop() {
    active = false;
    unlisten?.();
    unlisten = null;
    over.value = false;
  }
  onMounted(listen);
  onActivated(listen);
  onDeactivated(stop);
  onBeforeUnmount(stop);

  return { over };
}
