// The window ↔ filebridge-service link (ADR 0008). The Rust side connects, starts the service
// when needed and reconnects; this store only mirrors the state for the UI.
import { defineStore } from "pinia";
import { ref } from "vue";
import { api, commands, errorMessage, events, ServiceError } from "@/lib/api";

export const useConnection = defineStore("connection", () => {
  /** `null` while the first connection attempt runs. */
  const connected = ref<boolean | null>(null);
  const endpoint = ref("");
  const version = ref<string | null>(null);
  const lastError = ref<string | null>(null);
  /** `incompatible_service`, `service_unavailable`, … (crates/ipc/README.md). */
  const lastCode = ref<string | null>(null);
  const checking = ref(false);
  const reconnectHooks: Array<() => void> = [];
  let started = false;

  /** Up again after being down: events sent meanwhile are lost, so the stores reload. Whichever
   *  notices first (the event or a ping) runs the hooks, once. */
  function up() {
    const was = connected.value;
    connected.value = true;
    lastError.value = null;
    lastCode.value = null;
    if (was === false) reconnectHooks.forEach((fn) => fn());
  }

  async function check() {
    checking.value = true;
    try {
      const pong = await api.ping();
      version.value = pong.version;
      up();
    } catch (e) {
      connected.value = false;
      lastError.value = errorMessage(e);
      lastCode.value = e instanceof ServiceError ? e.code : null;
    } finally {
      checking.value = false;
    }
  }

  async function init() {
    if (started) return;
    started = true;
    endpoint.value = await commands.serviceEndpoint();
    await events.connectionChanged.listen((e) => {
      endpoint.value = e.payload.endpoint;
      if (e.payload.connected) {
        up();
        void check();
      } else {
        connected.value = false;
      }
    });
    await check();
  }

  function onReconnect(fn: () => void) {
    reconnectHooks.push(fn);
  }

  return { connected, endpoint, version, lastError, lastCode, checking, init, check, onReconnect };
});
