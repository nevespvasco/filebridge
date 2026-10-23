// Quick ways to a source (docs/design/specs/D02): folders the user pinned, and the disks mounted
// on the service's computer. Shared by step 1 and Home.
import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { PinnedSource, VolumeView_Serialize } from "@/bindings";
import { api, errorMessage } from "@/lib/api";
import { canListVolumes } from "@/lib/platform";

export const useSourceShortcuts = defineStore("sourceShortcuts", () => {
  const pinned = ref<PinnedSource[]>([]);
  const volumes = ref<VolumeView_Serialize[]>([]);
  const pinError = ref<string | null>(null);
  const pinnedPaths = computed(() => new Set(pinned.value.map((p) => p.path)));

  /** Optional sections: a failed load hides them rather than showing an error. */
  async function loadPinned() {
    try {
      pinned.value = await api.pinnedSources();
    } catch {
      pinned.value = [];
    }
  }

  async function loadVolumes() {
    if (!canListVolumes()) {
      volumes.value = [];
      return;
    }
    try {
      volumes.value = await api.volumes();
    } catch {
      volumes.value = [];
    }
  }

  function isPinned(path: string): boolean {
    return pinnedPaths.value.has(path);
  }

  async function setPinned(path: string, on: boolean) {
    pinError.value = null;
    try {
      await api.pinSource(path, on);
    } catch (e) {
      pinError.value = errorMessage(e);
    }
    await loadPinned();
  }

  return { pinned, volumes, pinError, isPinned, loadPinned, loadVolumes, setPinned };
});
