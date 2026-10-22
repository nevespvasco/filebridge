// Saved destinations, their connection tests and the Nextcloud direct-mode detection.
import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import type { DetectNextcloudRequest_Deserialize, DetectNextcloudResponse_Serialize, NextcloudWebdavRequest, Profile_Deserialize, Profile_Serialize, ProfileTest_Serialize, StorageView_Serialize as StorageView } from "@/bindings";
import { api, errorMessage } from "@/lib/api";

export type TestState = { running: boolean; result?: ProfileTest_Serialize; error?: string };

export const useProfiles = defineStore("profiles", () => {
  // shallowRef: `integration_settings` is a recursive JSON type, too deep for `ref` to unwrap.
  const list = shallowRef<Profile_Serialize[]>([]);
  const loaded = ref(false);
  const loadError = ref<string | null>(null);
  const tests = ref<Record<string, TestState>>({});
  /** Each destination's space from its last check (docs/design/specs/D03). */
  const storage = ref<Record<string, StorageView>>({});
  const checking = ref<Record<string, boolean>>({});

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
    void loadStorage(id);
    return tests.value[id];
  }

  /** The space from the last check; `refresh` checks first (a network round trip). */
  async function loadStorage(id: string, refresh = false): Promise<StorageView | undefined> {
    if (refresh) checking.value[id] = true;
    try {
      storage.value[id] = await api.profileStorage(id, refresh);
    } catch {
      // Keep what was shown.
    } finally {
      if (refresh) checking.value[id] = false;
    }
    return storage.value[id];
  }

  async function loadAllStorage() {
    await Promise.all(list.value.map((p) => loadStorage(p.id)));
  }

  /** Destinations under the low-space thresholds. */
  const low = computed(() => list.value.filter((p) => storage.value[p.id]?.low));

  async function detectNextcloud(req: DetectNextcloudRequest_Deserialize): Promise<DetectNextcloudResponse_Serialize> {
    const res = await api.detectNextcloud(req);
    if (res.profile) await load();
    return res;
  }

  /** WebDAV-mode wizard (docs/05): the app password goes to the keyring, never into a profile. */
  async function addNextcloudWebdav(req: NextcloudWebdavRequest): Promise<DetectNextcloudResponse_Serialize> {
    const res = await api.addNextcloudWebdav(req);
    if (res.profile) await load();
    return res;
  }

  return { list, loaded, loadError, tests, storage, checking, low, byId, load, ensure, save, remove, test, loadStorage, loadAllStorage, detectNextcloud, addNextcloudWebdav };
});

/** Free space reported by a connection test, if any. */
export function freeBytes(t: ProfileTest_Serialize | undefined): number | null {
  return t?.transport.free_bytes ?? null;
}

/** What kind of destination a profile is, for labels and icons. */
export function profileKind(p: Profile_Serialize): "local" | "sftp" | "webdav" | "nextcloud" {
  if (p.integration === "nextcloud") return "nextcloud";
  return p.destination.backend.kind;
}

/** How a send reaches the destination, for the Review step: Nextcloud's two modes differ. */
export function sendMode(p: Profile_Serialize): "local" | "sftp" | "webdav" | "nextcloud" | "nextcloudWebdav" {
  if (p.integration === "nextcloud" && p.destination.backend.kind === "webdav") return "nextcloudWebdav";
  return profileKind(p);
}

export function profileWhere(p: Profile_Serialize): string {
  const b = p.destination.backend;
  if (b.kind === "sftp") {
    const user = b.user ? `${b.user}@` : "";
    const port = b.port && b.port !== 22 ? `:${b.port}` : "";
    return `${user}${b.host}${port}:${p.destination.root}`;
  }
  if (b.kind === "webdav") return `${b.url}${p.destination.root}`;
  return p.destination.root;
}
