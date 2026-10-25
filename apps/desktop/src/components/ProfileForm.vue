<script setup lang="ts">
// New destination: a local folder, an SFTP server, an SMB share or an S3 bucket (ADR 0022),
// Nextcloud (its own form), or an Immich external library fed over SFTP or into a local folder
// (ADR 0020); or an existing one to edit (`profile`). Editing keeps the destination's id, so its
// history stays with it; a saved password is never read back, only replaced.
import AppIcon from "@/components/AppIcon.vue";
import { computed, onMounted, reactive, ref, shallowRef, watch } from "vue";
import type { Backend_Deserialize, DetectNextcloudResponse_Serialize, Family, LocalTargetProbe, Profile_Deserialize, Profile_Serialize } from "@/bindings";
import FolderBrowser from "./FolderBrowser.vue";
import FormField from "./FormField.vue";
import { api, errorMessage } from "@/lib/api";
import { isMobile, pickPaths } from "@/lib/platform";
import { useProfiles } from "@/stores/profiles";
import { useI18n } from "vue-i18n";
import HelpTip from "./HelpTip.vue";
import { jobsUsingProfile, profileKind } from "@/stores/profiles";
import HostKeyPrompt from "./HostKeyPrompt.vue";
import ImmichFields, { type ImmichForm } from "./ImmichFields.vue";
import NextcloudDetect from "./NextcloudDetect.vue";
import NextcloudModeAdvisor, { type AdvisedMode, type AdvisorFields } from "./NextcloudModeAdvisor.vue";
import NextcloudWebdav from "./NextcloudWebdav.vue";
import S3Fields, { type S3Form } from "./S3Fields.vue";
import SegmentedControl from "./SegmentedControl.vue";
import SmbFields, { type SmbForm } from "./SmbFields.vue";

const props = defineProps<{ profile?: Profile_Serialize }>();
const emit = defineEmits<{ saved: [profile: Profile_Serialize]; cancel: [] }>();
const profiles = useProfiles();
const { t } = useI18n();

type Kind = "local" | "sftp" | "smb" | "s3" | "nextcloud" | "immich";
const editing = computed(() => !!props.profile);
/** Phones' rclone has no S3 backend (ADR 0022). */
const kinds = computed<Kind[]>(() => (isMobile() ? ["local", "sftp", "smb", "nextcloud", "immich"] : ["local", "sftp", "smb", "s3", "nextcloud", "immich"]));
const kind = ref<Kind>(props.profile ? (profileKind(props.profile) as Kind) : "local");
/** Arrow keys move the choice between kinds, as in any radio group. */
function stepKind(step: number) {
  const list = kinds.value;
  pickKind(list[(list.indexOf(kind.value) + step + list.length) % list.length]);
}
/** Only a local folder or an SFTP server is edited in full; the others by name only: their
 *  wizards checked the rest together. */
const nameOnly = computed(() => !!props.profile && !["local", "sftp"].includes(profileKind(props.profile)));
/** A share or a bucket: their own fields, and a folder inside them instead of a full path. */
const share = computed(() => kind.value === "smb" || kind.value === "s3");
const smb = reactive<SmbForm>({ host: "", port: 445, share: "", user: "", domain: "" });
const s3 = reactive<S3Form>({ endpoint: "", region: "", provider: "Other", bucket: "", accessKeyId: "", pathStyle: true });
/** How files reach Immich's library folder: over SFTP, or into a folder of this computer. */
const immichVia = ref<"sftp" | "local">("sftp");
const immichVias = computed(() => [
  { value: "sftp" as const, label: t("immich.viaSftp"), icon: "sftp" as const },
  { value: "local" as const, label: t("immich.viaLocal"), icon: "local" as const },
]);
/** Which destination fields the form shows: an SFTP server or a local folder (Immich has both). */
const sftp = computed(() => kind.value === "sftp" || (kind.value === "immich" && immichVia.value === "sftp"));
const local = computed(() => !sftp.value && !share.value);
const immich = reactive<ImmichForm>({ server: "", importPath: "", libraryId: "" });
/** Kept apart from the form; goes to the keyring through the service and is cleared once saved. */
const apiKey = ref("");
const immichResult = shallowRef<DetectNextcloudResponse_Serialize | null>(null);
/** Nextcloud's two modes (docs/05): WebDAV by default, direct on the server for big loads. */
const ncMode = ref<"webdav" | "direct">("webdav");
/** What the mode advisor was given (ADR 0024), to fill in the chosen form; `ncKey` remounts the
 *  forms so they pick it up. */
const ncFields = shallowRef<AdvisorFields | null>(null);
const ncKey = ref(0);
function advised(mode: AdvisedMode, fields: AdvisorFields) {
  ncMode.value = mode;
  ncFields.value = fields;
  ncKey.value++;
}
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
/** SFTP and SMB's password, or S3's secret access key; optional. Goes to the OS keyring after the
 *  profile is saved, never into it. */
const password = ref("");
/** Editing a destination that has a password: it stays unless the user asks to replace it. */
const hasPassword = computed(() => {
  const b = props.profile?.destination.backend;
  return b?.kind === "sftp" && !!b.password;
});
const replacingPassword = ref(false);

// Editing: the form starts from the saved destination.
if (props.profile) {
  const p = props.profile;
  const b = p.destination.backend;
  form.name = p.name;
  form.root = p.destination.root;
  form.family = p.target.family;
  form.case_sensitive = p.target.case_sensitive;
  if (b.kind === "sftp") {
    form.host = b.host;
    form.port = b.port ?? 22;
    form.user = b.user ?? "";
    form.key_file = b.key_file ?? "";
    form.known_hosts_file = b.known_hosts_file ?? "";
  }
}
/** Whether the edit changes where files go (not just the name). */
const movesDestination = computed(() => {
  const p = props.profile;
  if (!p) return false;
  const b = p.destination.backend;
  const was = b.kind === "sftp" ? [b.host, b.port ?? 22, b.user ?? ""] : [];
  const now = kind.value === "sftp" ? [form.host.trim(), form.port, form.user.trim()] : [];
  return form.root.trim() !== p.destination.root || JSON.stringify(was) !== JSON.stringify(now) || form.family !== p.target.family || form.case_sensitive !== p.target.case_sensitive;
});
/** Transfers to this destination not finished yet: its place can't change under them. */
const unfinished = ref(0);
onMounted(async () => {
  if (props.profile) unfinished.value = await jobsUsingProfile(props.profile.id).catch(() => 0);
});
const blockedByJobs = computed(() => movesDestination.value && unfinished.value > 0);
/** The profile saved by a try whose password didn't reach the keyring: a retry saves it again
 *  under the same id (no second destination) and stores the password. */
const savedId = ref<string | null>(null);
const saving = ref(false);
const error = ref<string | null>(null);

/** What still stops the form from being saved, in words the user can act on. */
const missing = computed(() => {
  const m: string[] = [];
  if (!form.name.trim()) m.push(t("profile.name"));
  if (nameOnly.value) return m;
  if (sftp.value && !form.host.trim()) m.push(t("profile.host"));
  if (sftp.value && !(form.port > 0)) m.push(t("profile.port"));
  if (kind.value === "smb" && !smb.host.trim()) m.push(t("profile.host"));
  if (kind.value === "smb" && !smb.share.trim()) m.push(t("smb.share"));
  if (kind.value === "smb" && !(smb.port > 0)) m.push(t("profile.port"));
  if (kind.value === "s3" && !s3.bucket.trim()) m.push(t("s3.bucket"));
  if (kind.value === "s3" && s3.accessKeyId.trim() && !password.value) m.push(t("s3.secret"));
  if (!share.value && !form.root.trim()) m.push(local.value ? t("profile.folder") : t("profile.remoteFolder"));
  if (kind.value === "immich" && !immich.server.trim()) m.push(t("immich.server"));
  if (kind.value === "immich" && !apiKey.value.trim()) m.push(t("immich.apiKey"));
  return m;
});
const rootNotAbsolute = computed(() => !nameOnly.value && sftp.value && !!form.root.trim() && !form.root.trim().startsWith("/"));
const bucketHasSlash = computed(() => kind.value === "s3" && s3.bucket.trim().replace(/^\/+|\/+$/g, "").includes("/"));
const valid = computed(() => !missing.value.length && !rootNotAbsolute.value && !bucketHasSlash.value && !blockedByJobs.value);

/** Picking a kind: an SMB share is most often Windows or Samba with Windows' rules. */
function pickKind(k: Kind) {
  kind.value = k;
  if (k === "smb") setFamily("windows");
  else if (k === "s3") setFamily("unix");
}

function setFamily(f: Family) {
  form.family = f;
  // Windows and macOS file systems are case-insensitive by default.
  form.case_sensitive = f === "unix";
}

// A local folder's file system decides its name rules: filebridge asks the disk instead of the
// user ("Detected: exFAT disk, treated as Windows"), and the choice stays one click away.
const probe = ref<LocalTargetProbe | null>(null);
/** The user opened "Change": the radios show, and a detection no longer overrides them. */
const manualTarget = ref(false);
let probeTimer: ReturnType<typeof setTimeout> | undefined;
let probeSeq = 0;
watch(
  () => [local.value, form.root.trim()] as const,
  ([isLocal, root]) => {
    clearTimeout(probeTimer);
    probe.value = null;
    // Editing: the saved choice stands until the folder changes.
    if (!isLocal || !root || root === props.profile?.destination.root) return;
    const seq = ++probeSeq;
    probeTimer = setTimeout(async () => {
      try {
        const found = await api.probeLocalTarget(root);
        if (seq !== probeSeq) return;
        probe.value = found;
        if (found.family && !manualTarget.value) {
          form.family = found.family;
          form.case_sensitive = found.case_sensitive ?? found.family === "unix";
        }
      } catch {
        // No detection: the radios stay as they are.
      }
    }, 400);
  },
  { immediate: true },
);
const detected = computed(() => local.value && !!probe.value?.family && !manualTarget.value);

async function pick(field: "root" | "key_file") {
  const [p] = await pickPaths({ directory: field === "root", multiple: false });
  if (p) form[field] = p;
}

/** A trimmed text field, `null` when empty. */
function opt(v: string): string | null {
  return v.trim() || null;
}

/** The profile the form describes (not saved). Editing keeps what the form doesn't show: the
 *  keyring references (names, never secrets), the integration and the target's limits. */
function build(root: string): Profile_Deserialize {
  const p = props.profile;
  if (p && nameOnly.value) return { ...p, name: form.name.trim() };
  const old = p?.destination.backend;
  const backend: Backend_Deserialize =
    kind.value === "smb"
      ? {
          kind: "smb",
          host: smb.host.trim(),
          port: smb.port === 445 ? null : smb.port,
          user: opt(smb.user),
          domain: opt(smb.domain),
          share: smb.share.trim().replace(/^\/+|\/+$/g, ""),
          password: null,
        }
      : kind.value === "s3"
      ? {
          kind: "s3",
          provider: s3.provider === "Other" ? null : s3.provider,
          endpoint: opt(s3.endpoint),
          region: opt(s3.region),
          bucket: s3.bucket.trim().replace(/^\/+|\/+$/g, ""),
          access_key_id: opt(s3.accessKeyId),
          secret_access_key: null,
          path_style: s3.pathStyle,
        }
      : sftp.value
        ? {
            kind: "sftp",
            host: form.host.trim(),
            port: form.port === 22 ? null : form.port,
            user: form.user.trim() || null,
            key_file: form.key_file.trim() || null,
            known_hosts_file: form.known_hosts_file.trim() || null,
            password: old?.kind === "sftp" ? old.password : null,
            key: old?.kind === "sftp" ? old.key : null,
          }
        : { kind: "local" };
  return {
    id: p?.id ?? savedId.value ?? "",
    name: form.name.trim(),
    // A share's or a bucket's folder is relative to it.
    destination: { transport: "rclone", backend, root: share.value ? root.replace(/^\/+|\/+$/g, "") : root },
    integration: p?.integration ?? null,
    integration_settings: p?.integration_settings ?? null,
    target: { ...(p?.target ?? {}), family: form.family, case_sensitive: form.case_sensitive },
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
    if (sftp.value && !savedId.value && !nameOnly.value) {
      let known: boolean;
      try {
        known = await hostKnown();
      } catch (e) {
        error.value = t("hostKey.unreachable", { error: errorMessage(e) });
        return;
      }
      if (!known) return;
    }
    if (kind.value === "immich" && !editing.value) {
      await addImmich();
      return;
    }
    let saved = await profiles.save(build(form.root.trim()));
    // A password needs a user (SMB) and an S3 secret a key id: without one, there is no secret.
    const withSecret = kind.value === "sftp" || (kind.value === "smb" && !!smb.user.trim()) || (kind.value === "s3" && !!s3.accessKeyId.trim());
    if (!nameOnly.value && withSecret && password.value) {
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
    <h3 class="q-title">{{ editing ? $t("profile.editTitle", { name: profile!.name }) : $t("profile.kindQuestion") }}</h3>
    <div v-if="!editing" class="kinds" role="radiogroup" :aria-label="$t('profile.kind')">
      <button
        v-for="k in kinds"
        :key="k"
        type="button"
        role="radio"
        class="kind"
        :aria-checked="kind === k"
        :tabindex="kind === k ? 0 : -1"
        @click="pickKind(k)"
        @keydown.right.prevent="stepKind(1)"
        @keydown.down.prevent="stepKind(1)"
        @keydown.left.prevent="stepKind(-1)"
        @keydown.up.prevent="stepKind(-1)"
      >
        <strong><AppIcon :name="k" :size="20" /> {{ $t(`profile.kinds.${k}`) }}</strong>
        <span class="small muted">{{ $t(`profile.kindsHint.${k}`) }}</span>
      </button>
    </div>

    <template v-if="kind === 'nextcloud' && !editing">
      <div class="nc-mode">
        <SegmentedControl v-model="ncMode" :options="ncModes" :label="$t('webdav.mode')" />
        <span class="small muted">{{ $t(`webdav.modeHint.${ncMode}`) }}</span>
      </div>
      <NextcloudModeAdvisor @advised="advised" />
      <NextcloudWebdav v-if="ncMode === 'webdav'" :key="`w${ncKey}`" :initial-user="ncFields?.nextcloud_user" @saved="emit('saved', $event)" @cancel="emit('cancel')" />
      <NextcloudDetect v-else :key="`d${ncKey}`" :initial="ncFields" @saved="emit('saved', $event)" @cancel="emit('cancel')" />
    </template>

    <form v-else @submit.prevent="save">
      <div v-if="kind === 'immich' && !nameOnly" class="nc-mode">
        <p class="guide small"><AppIcon class="guide-icon" name="immich" :size="20" /><span>{{ $t("immich.intro") }}</span></p>
        <SegmentedControl v-model="immichVia" :options="immichVias" :label="$t('immich.via')" />
        <span class="small muted">{{ $t(`immich.viaHint.${immichVia}`) }}</span>
      </div>
      <div class="form-grid">
        <FormField v-slot="f" :label="$t('profile.name')" required :help="$t('profile.help.name')" :example="$t('profile.help.nameExample')" :hint="$t('profile.hint.name')">
          <input :id="f.id" v-model="form.name" required :aria-describedby="f.describedby" :placeholder="kind === 'immich' ? $t('immich.nameHint') : kind === 's3' ? $t('s3.nameHint') : local ? $t('profile.nameHintLocal') : $t('profile.nameHintSftp')" />
        </FormField>
        <p v-if="nameOnly" class="small muted wide">{{ $t("profile.editNameOnly") }}</p>
        <template v-if="sftp && !nameOnly">
          <FormField v-slot="f" :label="$t('profile.host')" required :help="$t('profile.help.host')" example="nas.local · 192.168.77.20" :hint="$t('profile.hint.host')">
            <input :id="f.id" v-model="form.host" required spellcheck="false" :aria-describedby="f.describedby" placeholder="nas.local" />
          </FormField>
          <FormField v-slot="f" class="narrow" :label="$t('profile.port')" :help="$t('profile.help.port')" example="22" :hint="$t('profile.hint.port')">
            <input :id="f.id" v-model.number="form.port" type="number" min="1" max="65535" :aria-describedby="f.describedby" />
          </FormField>
          <FormField v-slot="f" :label="$t('profile.user')" :help="$t('profile.help.user')" example="maria" :hint="$t('profile.hint.user')">
            <input :id="f.id" v-model="form.user" spellcheck="false" :aria-describedby="f.describedby" :placeholder="$t('common.optional')" />
          </FormField>
        </template>
        <template v-if="share && !nameOnly">
          <SmbFields v-if="kind === 'smb'" v-model:fields="smb" v-model:password="password" />
          <S3Fields v-if="kind === 's3'" v-model:fields="s3" v-model:secret="password" />
          <FormField
            v-slot="f"
            class="wide"
            :label="$t(`${kind}.folder`)"
            :help="$t(`${kind}.help.folder`)"
            :example="kind === 'smb' ? 'Fotos/2026' : 'backup/fotos'"
            :hint="$t(`${kind}.hint.folder`)"
          >
            <input :id="f.id" v-model="form.root" spellcheck="false" :aria-describedby="f.describedby" :placeholder="$t(`${kind}.folderHint`)" />
          </FormField>
        </template>
        <FormField
          v-else-if="!nameOnly"
          v-slot="f"
          class="wide"
          :label="local ? $t('profile.folder') : $t('profile.remoteFolder')"
          required
          :help="local ? $t('profile.help.folder') : $t('profile.help.remoteFolder')"
          :example="local ? '/media/disco/Backup · D:\\Backup' : '/volume1/backup · /srv/backup'"
          :hint="rootNotAbsolute ? $t('profile.slashRule') : local ? $t('profile.hint.folder') : $t('profile.hint.remoteFolder')"
          :hint-error="rootNotAbsolute"
        >
          <span class="with-button">
            <input :id="f.id" v-model="form.root" required spellcheck="false" :aria-describedby="f.describedby" :placeholder="local ? '/media/disco/Backup' : '/srv/backup'" />
            <button v-if="local" type="button" class="btn btn-small" :title="$t('profile.hint.browse')" @click="pick('root')"><AppIcon name="folder" /> {{ $t("common.browse") }}</button>
            <button v-else-if="sftp" type="button" class="btn btn-small" :disabled="!canBrowse || checkingHost" :title="canBrowse ? $t('profile.hint.browseServer') : $t('profile.hint.browseNeedsHost')" @click="openBrowser"><AppIcon name="folder" /> {{ $t("common.browse") }}</button>
          </span>
        </FormField>
        <template v-if="sftp && !nameOnly">
          <FormField v-slot="f" :label="$t('profile.keyFile')" :help="$t('profile.help.keyFile')" example="~/.ssh/id_ed25519" :hint="$t('profile.hint.keyFile')">
            <span class="with-button">
              <input :id="f.id" v-model="form.key_file" spellcheck="false" :aria-describedby="f.describedby" :placeholder="$t('profile.keyFileHint')" />
              <button type="button" class="btn btn-small" @click="pick('key_file')">{{ $t("common.browse") }}</button>
            </span>
          </FormField>
          <div v-if="hasPassword && !replacingPassword" class="field">
            <span class="label-line">{{ $t("profile.password") }}</span>
            <span class="with-button stored">
              <span class="small"><AppIcon name="lock" :size="16" /> {{ $t("profile.passwordStored") }}</span>
              <button type="button" class="btn btn-small" @click="replacingPassword = true">{{ $t("profile.passwordReplace") }}</button>
            </span>
          </div>
          <FormField v-else v-slot="f" :label="$t('profile.password')" :help="$t('profile.help.password')" :hint="$t('profile.hint.password')">
            <input :id="f.id" v-model="password" type="password" autocomplete="new-password" spellcheck="false" :aria-describedby="f.describedby" :placeholder="$t('profile.passwordHint')" />
          </FormField>
          <FormField v-slot="f" :label="$t('profile.knownHosts')" :help="$t('profile.help.knownHosts')" example="~/.ssh/known_hosts" :hint="$t('profile.hint.knownHosts')">
            <input :id="f.id" v-model="form.known_hosts_file" spellcheck="false" :aria-describedby="f.describedby" placeholder="~/.ssh/known_hosts" />
          </FormField>
          <p v-if="!editing" class="guide small wide"><AppIcon class="guide-icon" name="key" :size="20" /><span>{{ $t("profile.firstConnect") }}</span></p>
        </template>
        <ImmichFields v-if="kind === 'immich' && !nameOnly" v-model:fields="immich" v-model:api-key="apiKey" :result="immichResult" :folder="form.root" />
      </div>

      <p v-if="!nameOnly && detected" class="detected small" role="status">
        <AppIcon name="checkCircle" class="ok-text" />
        <span>{{ $t("profile.detected", { fs: probe!.fs_type ?? "?", family: $t(`profile.family.${form.family}`) }) }}
          <template v-if="!form.case_sensitive"> {{ $t("profile.detectedCase") }}</template></span>
        <button type="button" class="link" @click="manualTarget = true">{{ $t("profile.changeTarget") }}</button>
      </p>
      <fieldset v-else-if="!nameOnly && kind !== 'immich'" class="target">
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

      <p v-if="editing && movesDestination && !blockedByJobs" class="guide warn small" role="note">
        <AppIcon class="guide-icon" name="warning" :size="20" /><span>{{ $t("profile.editMoves") }}</span>
      </p>
      <p v-if="blockedByJobs" class="error small" role="alert">{{ $t("profile.editBlocked", { n: unfinished }, unfinished) }}</p>
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
          {{ editing ? $t("common.save") : kind === "immich" ? (saving ? $t("immich.checking") : $t("immich.check")) : $t("profile.save") }}
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
.profile-form { container-type: inline-size; }
.nc-mode { display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.35rem; margin: 0.5rem 0 1rem; }
.q-title { margin: 0 0 0.6rem; font-size: var(--fs-base); }
.kinds { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 0.5rem; margin-bottom: 1rem; }
.kind {
  all: unset;
  box-sizing: border-box;
  cursor: pointer;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  padding: 0.6rem 0.75rem;
  display: grid;
  gap: 0.2rem;
}
.kind[aria-checked="true"] { border-color: var(--accent); background: var(--accent-soft); }
.kind:focus-visible { outline: 2px solid var(--accent); }
.with-button { display: flex; gap: 0.5rem; }
.with-button input { flex: 1; }
.target { border: 1px solid var(--border); border-radius: var(--radius-md); margin: 1rem 0 0; display: flex; gap: 0.5rem 1rem; flex-wrap: wrap; align-items: center; }
.target legend { display: inline-flex; align-items: center; }
.target .hint { flex-basis: 100%; margin: 0; }
.wide.guide, .guide.warn { margin: 0; }
.guide.warn { margin-top: var(--sp-3); }
.stored { align-items: center; justify-content: space-between; padding: var(--sp-2) 0; }
.radio { display: inline-flex; gap: 0.35rem; align-items: center; }
.detected { display: flex; gap: var(--sp-2); align-items: center; flex-wrap: wrap; margin: var(--sp-4) 0 0; }
.link { all: unset; cursor: pointer; color: var(--accent); }
.link:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.actions { display: flex; justify-content: flex-end; align-items: center; gap: 0.5rem; margin-top: 1rem; flex-wrap: wrap; }
/* The form's own width, not the window's: beside the sidebar a 900px window leaves it 600px. */
@container (max-width: 56rem) { .kinds { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
@container (max-width: 40rem) { .kinds { grid-template-columns: minmax(0, 1fr); } }
</style>
