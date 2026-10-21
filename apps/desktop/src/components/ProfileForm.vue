<script setup lang="ts">
// New destination: a local folder, an SFTP server, or Nextcloud in direct mode.
import { computed, reactive, ref } from "vue";
import type { Backend_Deserialize, Family, Profile_Serialize } from "@/bindings";
import { errorMessage } from "@/lib/api";
import { pickPaths } from "@/lib/platform";
import { useProfiles } from "@/stores/profiles";
import NextcloudDetect from "./NextcloudDetect.vue";

const emit = defineEmits<{ saved: [profile: Profile_Serialize]; cancel: [] }>();
const profiles = useProfiles();

type Kind = "local" | "sftp" | "nextcloud";
const kind = ref<Kind>("local");
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
const saving = ref(false);
const error = ref<string | null>(null);

const valid = computed(() => {
  if (!form.name.trim() || !form.root.trim()) return false;
  if (kind.value === "sftp") return !!form.host.trim() && form.root.trim().startsWith("/") && form.port > 0;
  return true;
});

function setFamily(f: Family) {
  form.family = f;
  // Windows and macOS file systems are case-insensitive by default.
  form.case_sensitive = f === "unix";
}

async function pick(field: "root" | "key_file") {
  const [p] = await pickPaths({ directory: field === "root", multiple: false });
  if (p) form[field] = p;
}

async function save() {
  saving.value = true;
  error.value = null;
  try {
    const backend: Backend_Deserialize =
      kind.value === "sftp"
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
    const saved = await profiles.save({
      id: "",
      name: form.name.trim(),
      destination: { transport: "rclone", backend, root: form.root.trim() },
      integration: null,
      integration_settings: null,
      target: { family: form.family, case_sensitive: form.case_sensitive },
    });
    emit("saved", saved);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="profile-form card">
    <div class="kinds" role="radiogroup" :aria-label="$t('profile.kind')">
      <button
        v-for="k in (['local', 'sftp', 'nextcloud'] as const)"
        :key="k"
        type="button"
        role="radio"
        class="kind"
        :aria-checked="kind === k"
        @click="kind = k"
      >
        <strong>{{ $t(`profile.kinds.${k}`) }}</strong>
        <span class="small muted">{{ $t(`profile.kindsHint.${k}`) }}</span>
      </button>
    </div>

    <NextcloudDetect v-if="kind === 'nextcloud'" @saved="emit('saved', $event)" @cancel="emit('cancel')" />

    <form v-else @submit.prevent="save">
      <div class="grid">
        <label class="field">
          <span>{{ $t("profile.name") }}</span>
          <input v-model="form.name" required :placeholder="kind === 'local' ? $t('profile.nameHintLocal') : $t('profile.nameHintSftp')" />
        </label>
        <template v-if="kind === 'sftp'">
          <label class="field">
            <span>{{ $t("profile.host") }}</span>
            <input v-model="form.host" required spellcheck="false" placeholder="nas.lan" />
          </label>
          <label class="field narrow">
            <span>{{ $t("profile.port") }}</span>
            <input v-model.number="form.port" type="number" min="1" max="65535" />
          </label>
          <label class="field">
            <span>{{ $t("profile.user") }}</span>
            <input v-model="form.user" spellcheck="false" />
          </label>
        </template>
        <label class="field wide">
          <span>{{ kind === "local" ? $t("profile.folder") : $t("profile.remoteFolder") }}</span>
          <span class="with-button">
            <input v-model="form.root" required spellcheck="false" :placeholder="kind === 'local' ? '/media/disco/Backup' : '/srv/backup'" />
            <button v-if="kind === 'local'" type="button" class="btn btn-small" @click="pick('root')">{{ $t("common.browse") }}</button>
          </span>
        </label>
        <template v-if="kind === 'sftp'">
          <label class="field wide">
            <span>{{ $t("profile.keyFile") }}</span>
            <span class="with-button">
              <input v-model="form.key_file" spellcheck="false" :placeholder="$t('profile.keyFileHint')" />
              <button type="button" class="btn btn-small" @click="pick('key_file')">{{ $t("common.browse") }}</button>
            </span>
          </label>
          <label class="field wide">
            <span>{{ $t("profile.knownHosts") }}</span>
            <input v-model="form.known_hosts_file" spellcheck="false" placeholder="~/.ssh/known_hosts" />
          </label>
          <p class="small muted wide">{{ $t("profile.noPassword") }}</p>
        </template>
      </div>

      <fieldset class="target">
        <legend class="small">{{ $t("profile.target") }}</legend>
        <label v-for="f in (['unix', 'windows', 'macos'] as const)" :key="f" class="radio">
          <input type="radio" name="family" :checked="form.family === f" @change="setFamily(f)" />
          {{ $t(`profile.family.${f}`) }}
        </label>
        <label class="radio">
          <input v-model="form.case_sensitive" type="checkbox" />
          {{ $t("profile.caseSensitive") }}
        </label>
      </fieldset>

      <p v-if="error" class="error small">{{ error }}</p>
      <div class="actions">
        <button type="button" class="btn" @click="emit('cancel')">{{ $t("common.cancel") }}</button>
        <button type="submit" class="btn btn-primary" :disabled="!valid || saving">{{ $t("profile.save") }}</button>
      </div>
    </form>
  </div>
</template>

<style scoped>
.kinds { display: grid; grid-template-columns: repeat(3, 1fr); gap: 0.5rem; margin-bottom: 1rem; }
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
.target { border: 1px solid var(--border); border-radius: 8px; margin: 1rem 0 0; display: flex; gap: 1rem; flex-wrap: wrap; }
.radio { display: inline-flex; gap: 0.35rem; align-items: center; }
.actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 1rem; }
@media (max-width: 700px) { .kinds { grid-template-columns: 1fr; } }
</style>
