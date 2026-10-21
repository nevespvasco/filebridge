// The window ↔ filebridge-service link (ADR 0008). The Rust side connects, starts the service
// when needed and reconnects; this store only mirrors the state for the UI.
import { defineStore } from "pinia";
import { ref } from "vue";
import { api, commands, errorMessage, events } from "@/lib/api";

export const useConnection = defineStore("connection", () => {
  /** `null` while the first connection attempt runs. */
  const connected = ref<boolean | null>(null);
  const endpoint = ref("");
  const version = ref<string | null>(null);
  const lastError = ref<string | null>(null);
  const reconnectHooks: Array<() => void> = [];
  let started = false;

  async function check() {
    try {
      const pong = await api.ping();
      version.value = pong.version;
      connected.value = true;
      lastError.value = null;
    } catch (e) {
      connected.value = false;
      lastError.value = errorMessage(e);
    }
  }

  async function init() {
    if (started) return;
    started = true;
    endpoint.value = await commands.serviceEndpoint();
    await events.connectionChanged.listen((e) => {
      const was = connected.value;
      connected.value = e.payload.connected;
      endpoint.value = e.payload.endpoint;
      if (e.payload.connected) {
        lastError.value = null;
        void check();
        // Events sent while disconnected are lost: let the stores reload.
        if (was === false) reconnectHooks.forEach((fn) => fn());
      }
    });
    await check();
  }

  function onReconnect(fn: () => void) {
    reconnectHooks.push(fn);
  }

  return { connected, endpoint, version, lastError, init, check, onReconnect };
});
