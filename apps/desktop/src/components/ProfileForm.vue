<script setup lang="ts">
// New destination: a local folder, an SFTP server, Nextcloud (its own form), or an Immich
// external library fed over SFTP or into a local folder (ADR 0020).
import AppIcon from "@/components/AppIcon.vue";
import { computed, reactive, ref, shallowRef } from "vue";
import type { Backend_Deserialize, DetectNextcloudResponse_Serialize, Family, Profile_Deserialize, Profile_Serialize } from "@/bindings";
import FolderBrowser from "./FolderBrowser.vue";
import { api, errorMessage } from "@/lib/api";
import { pickPaths } from "@/lib/platform";
import { useProfiles } from "@/stores/profiles";
import { useI18n } from "vue-i18n";
import HelpTip from "./HelpTip.vue";
import HostKeyPrompt from "./HostKeyPrompt.vue";
import ImmichFields, { type ImmichForm } from "./ImmichFields.vue";
import NextcloudDetect from "./NextcloudDetect.vue";
import NextcloudWebdav from "./NextcloudWebdav.vue";
import SegmentedControl from "./SegmentedControl.vue";

const emit = defineEmits<{ saved: [profile: Profile_Serialize]; cancel: [] }>();
const profiles = useProfiles();
const { t } = useI18n();

type Kind = "local" | "sftp" | "nextcloud" | "immich";
const kind = ref<Kind>("local");
/** How files reach Immich's library folder: over SFTP, or into a folder of this computer. */
const immichVia = ref<"sftp" | "local">("sftp");
const immichVias = computed(() => [
  { value: "sftp" as const, label: t("immich.viaSftp"), icon: "sftp" as const },
  { value: "local" as const, label: t("immich.viaLocal"), icon: "local" as const },
]);
/** Which destination fields the form shows: an SFTP server or a local folder (Immich has both). */
const sftp = computed(() => kind.value === "sftp" || (kind.value === "immich" && immichVia.value === "sftp"));
const local = computed(() => !sftp.value);
const immich = reactive<ImmichForm>({ server: "", importPath: "", libraryId: "" });
/** Kept apart from the form; goes to the keyring through the service and is cleared once saved. */
const apiKey = ref("");
const immichResult = shallowRef<DetectNextcloudResponse_Serialize | null>(null);
/** Nextcloud's two modes (docs/05): WebDAV by default, direct on the server for big loads. */
const ncMode = ref<"webdav" | "direct">("webdav");
const ncModes = computed(() => [
  { value: "webdav" as const, label: t("webdav.modeWebdav"), icon: "nextcloud" as const },
  { value: "direct" as const, label: t("webdav.modeDirect"), icon: "sftp" as const },
]);
const form = reactive({
  name: "",
  root: "",
  host: "",
  port: 22,
  user: "",
  key_file: "",
  known_hosts_file: "",
  family: "unix" as Family,
  case_sensitive: true,
});
/** SFTP only, optional; goes to the OS keyring after the profile is saved, never into it. */
const password = ref("");
/** The profile saved by a try whose password didn't reach the keyring: a retry saves it again
 *  under the same id (no second destination) and stores the password. */
const savedId = ref<string | null>(null);
const saving = ref(false);
const error = ref<string | null>(null);

/** What still stops the form from being saved, in words the user can act on. */
const missing = computed(() => {
  const m: string[] = [];
  if (!form.name.trim()) m.push(t("profile.name"));
  if (sftp.value && !form.host.trim()) m.push(t("profile.host"));
  if (sftp.value && !(form.port > 0)) m.push(t("profile.port"));
  if (!form.root.trim()) m.push(local.value ? t("profile.folder") : t("profile.remoteFolder"));
  if (kind.value === "immich" && !immich.server.trim()) m.push(t("immich.server"));
  if (kind.value === "immich" && !apiKey.value.trim()) m.push(t("immich.apiKey"));
  return m;
});
const rootNotAbsolute = computed(() => sftp.value && !!form.root.trim() && !form.root.trim().startsWith("/"));
const valid = computed(() => !missing.value.length && !rootNotAbsolute.value);

function setFamily(f: Family) {
  form.family = f;
  // Windows and macOS file systems are case-insensitive by default.
  form.case_sensitive = f === "unix";
}

async function pick(field: "root" | "key_file") {
  const [p] = await pickPaths({ directory: field === "root", multiple: false });
  if (p) form[field] = p;
}

/** The profile the form describes (not saved). */
function build(root: string): Profile_Deserialize {
  const backend: Backend_Deserialize =
      sftp.value
        ? {
            kind: "sftp",
            host: form.host.trim(),
            port: form.port === 22 ? null : form.port,
            user: form.user.trim() || null,
            key_file: form.key_file.trim() || null,
            known_hosts_file: form.known_hosts_file.trim() || null,
            password: null,
          }
        : { kind: "local" };
  return {
    id: savedId.value ?? "",
    name: form.name.trim(),
    destination: { transport: "rclone", backend, root },
    integration: null,
    integration_settings: null,
    target: { family: form.family, case_sensitive: form.case_sensitive },
  };
}

// An SFTP server's host key must be in known_hosts before anything connects to it: an unknown
// one is shown to the user to trust (ADR 0015).
const hostKey = ref<InstanceType<typeof HostKeyPrompt> | null>(null);
function hostKnown(): Promise<boolean> {
  return hostKey.value!.ensureKnown(form.host.trim(), form.port, form.known_hosts_file.trim() || null);
}

// Browse the server for the root, before the profile exists (docs/design/specs/D11).
const browsing = ref(false);
const canBrowse = computed(() => sftp.value && !!form.host.trim());
const checkingHost = ref(false);
async function openBrowser() {
  checkingHost.value = true;
  error.value = null;
  try {
    if (await hostKnown()) browsing.value = true;
  } catch (e) {
    error.value = t("hostKey.unreachable", { error: errorMessage(e) });
  } finally {
    checkingHost.value = false;
  }
}
function chooseRoot(path: string) {
  form.root = `/${path}`;
  browsing.value = false;
}

/** The Immich wizard: the service checks the destination and Immich, and saves the profile
 *  (with the API key and any SFTP password in the keyring) only when nothing failed. */
async function addImmich() {
  immichResult.value = await profiles.addImmich({
    name: form.name.trim(),
    destination: build(form.root.trim()).destination,
    password: sftp.value && password.value ? password.value : null,
    server: immich.server.trim(),
    api_key: apiKey.value.trim(),
    library_id: immich.libraryId.trim() || null,
    import_path: immich.importPath.trim() || null,
  });
  if (immichResult.value.profile) {
    apiKey.value = "";
    password.value = "";
  }
}

async function save() {
  saving.value = true;
  error.value = null;
  try {
    if (sftp.value && !savedId.value) {
      let known: boolean;
      try {
        known = await hostKnown();
      } catch (e) {
        error.value = t("hostKey.unreachable", { error: errorMessage(e) });
        return;
      }
      if (!known) return;
    }
    if (kind.value === "immich") {
      await addImmich();
      return;
    }
    let saved = await profiles.save(build(form.root.trim()));
    if (kind.value === "sftp" && password.value) {
      savedId.value = saved.id;
      saved = await api.setProfilePassword(saved.id, password.value);
      password.value = "";
      await profiles.load();
    }
    savedId.value = null;
    emit("saved", saved);
  } catch (e) {
    error.value = savedId.value ? t("profile.passwordNotSaved", { error: errorMessage(e) }) : errorMessage(e);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="profile-form card">
    <h3 class="q-title">{{ $t("profile.kindQuestion") }}</h3>
    <div class="kinds" role="radiogroup" :aria-label="$t('profile.kind')">
      <button
        v-for="k in (['local', 'sftp', 'nextcloud', 'immich'] as const)"
        :key="k"
        type="button"
        role="radio"
        class="kind"
        :aria-checked="kind === k"
        @click="kind = k"
      >
        <strong><AppIcon :name="k" :size="20" /> {{ $t(`profile.kinds.${k}`) }}</strong>
        <span class="small muted">{{ $t(`profile.kindsHint.${k}`) }}</span>
      </button>
    </div>

    <template v-if="kind === 'nextcloud'">
      <div class="nc-mode">
        <SegmentedControl v-model="ncMode" :options="ncModes" :label="$t('webdav.mode')" />
        <span class="small muted">{{ $t(`webdav.modeHint.${ncMode}`) }}</span>
      </div>
      <NextcloudWebdav v-if="ncMode === 'webdav'" @saved="emit('saved', $event)" @cancel="emit('cancel')" />
      <NextcloudDetect v-else @saved="emit('saved', $event)" @cancel="emit('cancel')" />
    </template>

    <form v-else @submit.prevent="save">
      <div v-if="kind === 'immich'" class="nc-mode">
        <p class="guide small"><AppIcon class="guide-icon" name="immich" :size="20" /><span>{{ $t("immich.intro") }}</span></p>
        <SegmentedControl v-model="immichVia" :options="immichVias" :label="$t('immich.via')" />
        <span class="small muted">{{ $t(`immich.viaHint.${immichVia}`) }}</span>
      </div>
      <div class="grid">
        <label class="field">
          <span class="label-line">{{ $t("profile.name") }}<span class="req" aria-hidden="true">*</span>
            <HelpTip :text="$t('profile.help.name')" :example="$t('profile.help.nameExample')" :label="$t('profile.name')" />
          </span>
          <input v-model="form.name" required :placeholder="kind === 'immich' ? $t('immich.nameHint') : local ? $t('profile.nameHintLocal') : $t('profile.nameHintSftp')" />
          <span class="hint">{{ $t("profile.hint.name") }}</span>
        </label>
        <template v-if="sftp">
          <label class="field">
            <span class="label-line">{{ $t("profile.host") }}<span class="req" aria-hidden="true">*</span>
              <HelpTip :text="$t('profile.help.host')" example="nas.local · 192.168.77.20" :label="$t('profile.host')" />
            </span>
            <input v-model="form.host" required spellcheck="false" placeholder="nas.local" />
            <span class="hint">{{ $t("profile.hint.host") }}</span>
          </label>
          <label class="field narrow">
            <span class="label-line">{{ $t("profile.port") }}
              <HelpTip :text="$t('profile.help.port')" example="22" :label="$t('profile.port')" />
            </span>
            <input v-model.number="form.port" type="number" min="1" max="65535" />
            <span class="hint">{{ $t("profile.hint.port") }}</span>
          </label>
          <label class="field">
            <span class="label-line">{{ $t("profile.user") }}
              <HelpTip :text="$t('profile.help.user')" example="maria" :label="$t('profile.user')" />
            </span>
            <input v-model="form.user" spellcheck="false" :placeholder="$t('common.optional')" />
            <span class="hint">{{ $t("profile.hint.user") }}</span>
          </label>
        </template>
        <label class="field wide">
          <span class="label-line">{{ local ? $t("profile.folder") : $t("profile.remoteFolder") }}<span class="req" aria-hidden="true">*</span>
            <HelpTip
              :text="local ? $t('profile.help.folder') : $t('profile.help.remoteFolder')"
              :example="local ? '/media/disco/Backup · D:\\Backup' : '/volume1/backup · /srv/backup'"
              :label="local ? $t('profile.folder') : $t('profile.remoteFolder')"
            />
          </span>
          <span class="with-button">
            <input v-model="form.root" required spellcheck="false" :placeholder="local ? '/media/disco/Backup' : '/srv/backup'" />
            <button v-if="local" type="button" class="btn btn-small" :title="$t('profile.hint.browse')" @click="pick('root')"><AppIcon name="folder" /> {{ $t("common.browse") }}</button>
            <button v-else-if="sftp" type="button" class="btn btn-small" :disabled="!canBrowse || checkingHost" :title="canBrowse ? $t('profile.hint.browseServer') : $t('profile.hint.browseNeedsHost')" @click="openBrowser"><AppIcon name="folder" /> {{ $t("common.browse") }}</button>
          </span>
          <span v-if="rootNotAbsolute" class="hint error">{{ $t("profile.slashRule") }}</span>
          <span v-else class="hint">{{ local ? $t("profile.hint.folder") : $t("profile.hint.remoteFolder") }}</span>
        </label>
        <template v-if="sftp">
          <label class="field wide">
            <span class="label-line">{{ $t("profile.keyFile") }}
              <HelpTip :text="$t('profile.help.keyFile')" example="~/.ssh/id_ed25519" :label="$t('profile.keyFile')" />
            </span>
            <span class="with-button">
              <input v-model="form.key_file" spellcheck="false" :placeholder="$t('profile.keyFileHint')" />
              <button type="button" class="btn btn-small" @click="pick('key_file')">{{ $t("common.browse") }}</button>
            </span>
            <span class="hint">{{ $t("profile.hint.keyFile") }}</span>
          </label>
          <label class="field wide">
            <span class="label-line">{{ $t("profile.knownHosts") }}
              <HelpTip :text="$t('profile.help.knownHosts')" example="~/.ssh/known_hosts" :label="$t('profile.knownHosts')" />
            </span>
            <input v-model="form.known_hosts_file" spellcheck="false" placeholder="~/.ssh/known_hosts" />
            <span class="hint">{{ $t("profile.hint.knownHosts") }}</span>
          </label>
          <label class="field wide">
            <span class="label-line">{{ $t("profile.password") }}
              <HelpTip :text="$t('profile.help.password')" :label="$t('profile.password')" />
            </span>
            <input v-model="password" type="password" autocomplete="new-password" spellcheck="false" :placeholder="$t('profile.passwordHint')" />
            <span class="hint"><AppIcon name="lock" :size="16" /> {{ $t("profile.hint.password") }}</span>
          </label>
          <p class="guide small wide"><AppIcon class="guide-icon" name="key" :size="20" /><span>{{ $t("profile.firstConnect") }}</span></p>
        </template>
        <ImmichFields v-if="kind === 'immich'" v-model:fields="immich" v-model:api-key="apiKey" :result="immichResult" :folder="form.root" />
      </div>

      <fieldset v-if="kind !== 'immich'" class="target">
        <legend class="small">
          {{ $t("profile.target") }}
          <HelpTip :text="$t('profile.help.target')" :label="$t('profile.target')" />
        </legend>
        <label v-for="f in (['unix', 'windows', 'macos'] as const)" :key="f" class="radio">
          <input type="radio" name="family" :checked="form.family === f" @change="setFamily(f)" />
          {{ $t(`profile.family.${f}`) }}
        </label>
        <label class="radio">
          <input v-model="form.case_sensitive" type="checkbox" />
          {{ $t("profile.caseSensitive") }}
          <HelpTip :text="$t('profile.help.case')" example="Foto.jpg ≠ foto.jpg" :label="$t('profile.caseSensitive')" />
        </label>
        <p class="hint small muted">{{ $t("profile.hint.target") }}</p>
      </fieldset>

      <p v-if="error" class="error small" role="alert">{{ error }}</p>
      <div v-if="immichResult?.profile" class="actions">
        <p class="ok-text small">{{ $t("nextcloud.saved", { name: immichResult.profile.name }) }}</p>
        <span class="grow" />
        <button type="button" class="btn btn-primary" @click="emit('saved', immichResult.profile!)">{{ $t("nextcloud.done") }}</button>
      </div>
      <div v-else class="actions">
        <span v-if="missing.length" class="why-disabled">{{ $t("profile.missing", { fields: missing.join(", ") }) }}</span>
        <span class="grow" />
        <button type="button" class="btn" @click="emit('cancel')">{{ $t("common.cancel") }}</button>
        <button type="submit" class="btn btn-primary" :disabled="!valid || saving">
          {{ kind === "immich" ? (saving ? $t("immich.checking") : $t("immich.check")) : $t("profile.save") }}
        </button>
      </div>
    </form>
    <FolderBrowser
      :target="browsing ? { kind: 'draft', profile: build('/') } : null"
      :title="$t('browse.title', { destination: form.host })"
      :start="form.root.replace(/^\/+/, '')"
      :choose-label="$t('browse.choose')"
      @choose="chooseRoot"
      @close="browsing = false"
    />
    <HostKeyPrompt ref="hostKey" />
  </div>
</template>

<style scoped>
.nc-mode { display: grid; gap: 0.35rem; margin: 0.5rem 0 1rem; }
.q-title { margin: 0 0 0.6rem; font-size: 1rem; }
.kinds { display: grid; grid-template-columns: repeat(4, 1fr); gap: 0.5rem; margin-bottom: 1rem; }
.kind {
  all: unset;
  box-sizing: border-box;
  cursor: pointer;
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 0.6rem 0.75rem;
  display: grid;
  gap: 0.2rem;
}
.kind[aria-checked="true"] { border-color: var(--accent); background: var(--accent-soft); }
.kind:focus-visible { outline: 2px solid var(--accent); }
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 0.75rem; }
.wide { grid-column: 1 / -1; }
.with-button { display: flex; gap: 0.5rem; }
.with-button input { flex: 1; }
.target { border: 1px solid var(--border); border-radius: 8px; margin: 1rem 0 0; display: flex; gap: 0.5rem 1rem; flex-wrap: wrap; align-items: center; }
.target legend { display: inline-flex; align-items: center; }
.target .hint { flex-basis: 100%; margin: 0; }
.wide.guide { margin: 0; }
.radio { display: inline-flex; gap: 0.35rem; align-items: center; }
.actions { display: flex; justify-content: flex-end; align-items: center; gap: 0.5rem; margin-top: 1rem; flex-wrap: wrap; }
@media (max-width: 900px) { .kinds { grid-template-columns: repeat(2, 1fr); } }
@media (max-width: 700px) { .kinds { grid-template-columns: 1fr; } }
</style>
