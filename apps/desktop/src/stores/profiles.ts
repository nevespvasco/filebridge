// Saved destinations, their connection tests and the Nextcloud direct-mode detection.
import { defineStore } from "pinia";
import { computed, ref, shallowRef } from "vue";
import type { DetectNextcloudRequest_Deserialize, DetectNextcloudResponse_Serialize, ImmichRequest_Deserialize, NextcloudAdvice, NextcloudWebdavRequest, Profile_Deserialize, Profile_Serialize, ProfileTest_Serialize, StorageView_Serialize as StorageView } from "@/bindings";
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

  /** Which Nextcloud mode suits a server (ADR 0024): detection over SSH, nothing saved. */
  async function probeNextcloud(req: DetectNextcloudRequest_Deserialize): Promise<NextcloudAdvice> {
    return api.probeNextcloud(req);
  }

  /** WebDAV-mode wizard (docs/05): the app password goes to the keyring, never into a profile. */
  async function addNextcloudWebdav(req: NextcloudWebdavRequest): Promise<DetectNextcloudResponse_Serialize> {
    const res = await api.addNextcloudWebdav(req);
    if (res.profile) await load();
    return res;
  }

  /** Immich wizard (ADR 0020): the API key (and an SFTP password) go to the keyring, never into a
   *  profile. */
  async function addImmich(req: ImmichRequest_Deserialize): Promise<DetectNextcloudResponse_Serialize> {
    const res = await api.addImmich(req);
    if (res.profile) await load();
    return res;
  }

  return { list, loaded, loadError, tests, storage, checking, low, byId, load, ensure, save, remove, test, loadStorage, loadAllStorage, detectNextcloud, probeNextcloud, addNextcloudWebdav, addImmich };
});

/** Free space reported by a connection test, if any. */
export function freeBytes(t: ProfileTest_Serialize | undefined): number | null {
  return t?.transport.free_bytes ?? null;
}

/** What kind of destination a profile is, for labels and icons. */
export function profileKind(p: Profile_Serialize): "local" | "sftp" | "webdav" | "nextcloud" | "immich" {
  if (p.integration === "nextcloud") return "nextcloud";
  if (p.integration === "immich") return "immich";
  return p.destination.backend.kind;
}

/** Every SSH account whose `~/.ssh/authorized_keys` filebridge's own key changes (ADR 0012):
 *  the SFTP user, and Nextcloud direct mode's SSH user when it differs. Mirrors `targets` in
 *  crates/host/src/access.rs. */
export function sshAccounts(p: Profile_Serialize): string[] {
  const b = p.destination.backend;
  if (b.kind !== "sftp" || !b.user) return [];
  const out = [`${b.user}@${b.host}`];
  const s = p.integration_settings;
  if (s && typeof s === "object" && !Array.isArray(s) && s.mode === "direct") {
    const ssh = s.ssh;
    if (ssh && typeof ssh === "object" && !Array.isArray(ssh) && typeof ssh.user === "string" && typeof ssh.host === "string") {
      const port = typeof ssh.port === "number" ? ssh.port : 22;
      const same = ssh.user === b.user && ssh.host === b.host && port === (b.port ?? 22);
      if (!same) out.push(`${ssh.user}@${ssh.host}`);
    }
  }
  return out;
}

export type SshHost = { host: string; port: number; knownHostsFile: string | null };

/** Every SSH server this destination signs in to, with the known_hosts file its key is checked
 *  against (ADR 0015): the SFTP server, and Nextcloud direct mode's SSH server when it differs.
 *  Follows `targets` in crates/host/src/access.rs, one entry per server and file. */
export function sshHosts(p: Profile_Serialize): SshHost[] {
  const b = p.destination.backend;
  if (b.kind !== "sftp") return [];
  const out: SshHost[] = [{ host: b.host, port: b.port ?? 22, knownHostsFile: b.known_hosts_file }];
  const s = p.integration_settings;
  if (s && typeof s === "object" && !Array.isArray(s) && s.mode === "direct") {
    const ssh = s.ssh;
    if (ssh && typeof ssh === "object" && !Array.isArray(ssh) && typeof ssh.host === "string") {
      const h: SshHost = {
        host: ssh.host,
        port: typeof ssh.port === "number" ? ssh.port : 22,
        knownHostsFile: typeof ssh.known_hosts_file === "string" ? ssh.known_hosts_file : null,
      };
      if (!out.some((o) => o.host === h.host && o.port === h.port && o.knownHostsFile === h.knownHostsFile)) out.push(h);
    }
  }
  return out;
}

/** filebridge's own SSH key for this destination: "install" when it can get one, "remove" when
 *  it has one, `null` otherwise. Not offered to SFTP profiles that sign in by password only
 *  (the key is installed through the key file or SSH agent), nor without an SSH user. */
export function sshKeyAction(p: Profile_Serialize): "install" | "remove" | null {
  const b = p.destination.backend;
  if (b.kind !== "sftp") return null;
  if (b.key) return "remove";
  if (!b.user || (b.password && !b.key_file)) return null;
  return "install";
}

/** How a send reaches the destination, for the Review step: Nextcloud's two modes differ. */
export function sendMode(p: Profile_Serialize): "local" | "sftp" | "webdav" | "nextcloud" | "nextcloudWebdav" | "immich" {
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
