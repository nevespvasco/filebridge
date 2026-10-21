// Saved destinations, their connection tests and the Nextcloud direct-mode detection.
import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import type {
  DetectNextcloudRequest_Deserialize,
  DetectNextcloudResponse_Serialize,
  Profile_Deserialize,
  Profile_Serialize,
  ProfileTest_Serialize,
} from "@/bindings";
import { api, errorMessage } from "@/lib/api";

export type TestState = { running: boolean; result?: ProfileTest_Serialize; error?: string };

export const useProfiles = defineStore("profiles", () => {
  // shallowRef: `integration_settings` is a recursive JSON type, too deep for `ref` to unwrap.
  const list = shallowRef<Profile_Serialize[]>([]);
  const loaded = ref(false);
  const loadError = ref<string | null>(null);
  const tests = ref<Record<string, TestState>>({});

  const byId = computed(() => Object.fromEntries(list.value.map((p) => [p.id, p])));

  async function load() {
    try {
      list.value = await api.listProfiles();
      loadError.value = null;
    } catch (e) {
      loadError.value = errorMessage(e);
    } finally {
      loaded.value = true;
    }
  }

  /** Reloads the list when `id` isn't in it (created elsewhere, e.g. by the CLI). */
  async function ensure(id: string | null | undefined) {
    if (id && !byId.value[id]) await load();
  }

  async function save(profile: Profile_Deserialize): Promise<Profile_Serialize> {
    const saved = await api.saveProfile(profile);
    await load();
    return saved;
  }

  async function remove(id: string) {
    await api.deleteProfile(id);
    delete tests.value[id];
    await load();
  }

  async function test(id: string): Promise<TestState> {
    tests.value[id] = { running: true };
    try {
      tests.value[id] = { running: false, result: await api.testProfile(id) };
    } catch (e) {
      tests.value[id] = { running: false, error: errorMessage(e) };
    }
    return tests.value[id];
  }

  async function detectNextcloud(req: DetectNextcloudRequest_Deserialize): Promise<DetectNextcloudResponse_Serialize> {
    const res = await api.detectNextcloud(req);
    if (res.profile) await load();
    return res;
  }

  return { list, loaded, loadError, tests, byId, load, ensure, save, remove, test, detectNextcloud };
});

/** Free space reported by a connection test, if any. */
export function freeBytes(t: ProfileTest_Serialize | undefined): number | null {
  return t?.transport.free_bytes ?? null;
}

/** What kind of destination a profile is, for labels and icons. */
export function profileKind(p: Profile_Serialize): "local" | "sftp" | "nextcloud" {
  if (p.integration === "nextcloud") return "nextcloud";
  return p.destination.backend.kind;
}

export function profileWhere(p: Profile_Serialize): string {
  const b = p.destination.backend;
  if (b.kind === "sftp") {
    const user = b.user ? `${b.user}@` : "";
    const port = b.port && b.port !== 22 ? `:${b.port}` : "";
    return `${user}${b.host}${port}:${p.destination.root}`;
  }
  return p.destination.root;
}
