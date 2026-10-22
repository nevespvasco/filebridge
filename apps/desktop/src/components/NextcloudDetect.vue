<script setup lang="ts">
// Nextcloud direct mode (docs/05): SSH to the Proxmox host (and find the Nextcloud container) or
// to the machine Nextcloud runs on, then find its data directory, `occ` and the file owner. Each
// step shows ✓ / ! / ✗, and the Details panel the commands it ran.
import { trimBoth } from "@/lib/format";
import AppIcon from "@/components/AppIcon.vue";
import { computed, reactive, ref, shallowRef } from "vue";
import { useI18n } from "vue-i18n";
import type { DetectNextcloudResponse_Serialize, Profile_Serialize } from "@/bindings";
import { errorMessage } from "@/lib/api";
import { pickPaths } from "@/lib/platform";
import { useProfiles } from "@/stores/profiles";
import StatusIcon from "./StatusIcon.vue";
import DetailsPanel from "./DetailsPanel.vue";
import HelpTip from "./HelpTip.vue";

const emit = defineEmits<{ saved: [profile: Profile_Serialize]; cancel: [] }>();
const { t, te } = useI18n();
const profiles = useProfiles();

const form = reactive({
  name: "Nextcloud",
  host: "",
  port: 22,
  user: "root",
  key_file: "",
  known_hosts_file: "",
  nextcloud_user: "",
  folder: "",
  ctid: "",
  install_dir: "",
});
const running = ref(false);
const error = ref<string | null>(null);
const result = shallowRef<DetectNextcloudResponse_Serialize | null>(null);
const advanced = ref(false);

const missing = computed(() => {
  const m: string[] = [];
  if (!form.name.trim()) m.push(t("profile.name"));
  if (!form.nextcloud_user.trim()) m.push(t("nextcloud.ncUser"));
  if (!form.host.trim()) m.push(t("nextcloud.host"));
  if (!(form.port > 0)) m.push(t("profile.port"));
  if (!form.user.trim()) m.push(t("nextcloud.sshUser"));
  return m;
});
const valid = computed(() => !missing.value.length);
const commands = computed(() => result.value?.steps.flatMap((s) => [`# ${s.id}`, ...s.commands]) ?? []);
// The folder is relative to the Nextcloud user's files; an absolute path under a usual server
// mount point (e.g. /mnt/pve/data/…) is almost certainly the data folder on the host instead.
const folderLooksLikeHostPath = computed(() =>
  /^\/(mnt|srv|var|home|media|opt|data|tank|rpool|storage|root|usr|etc)(\/|$)/i.test(form.folder.trim()),
);
const runtimeKey = computed(() => `nextcloud.runtime.${result.value?.detector ?? ""}`);

function stepLabel(id: string) {
  const key = `nextcloud.steps.${id}`;
  return te(key) ? t(key) : id;
}

async function pickKey() {
  const [p] = await pickPaths({ directory: false, multiple: false });
  if (p) form.key_file = p;
}

async function detect() {
  running.value = true;
  error.value = null;
  result.value = null;
  try {
    const ctid = form.ctid.trim() ? Number(form.ctid) : null;
    result.value = await profiles.detectNextcloud({
      name: form.name.trim(),
      host: form.host.trim(),
      port: form.port,
      user: form.user.trim(),
      key_file: form.key_file.trim() || null,
      known_hosts_file: form.known_hosts_file.trim() || null,
      nextcloud_user: form.nextcloud_user.trim(),
      folder: trimBoth(form.folder.trim(), "/"),
      ctid: ctid != null && Number.isInteger(ctid) ? ctid : null,
      install_dir: form.install_dir.trim() || null,
    });
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    running.value = false;
  }
}

function chooseContainer(c: string) {
  form.ctid = c.split(/\s+/)[0] ?? c;
  void detect();
}
</script>

<template>
  <form class="nc" @submit.prevent="detect">
    <p class="small muted">{{ $t("nextcloud.intro") }}</p>
    <div class="grid">
      <label class="field">
        <span class="label-line">{{ $t("profile.name") }}<span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('profile.help.name')" :label="$t('profile.name')" />
        </span>
        <input v-model="form.name" required />
      </label>
      <label class="field">
        <span class="label-line">{{ $t("nextcloud.ncUser") }}<span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('nextcloud.help.ncUser')" example="maria" :label="$t('nextcloud.ncUser')" />
        </span>
        <input v-model="form.nextcloud_user" required spellcheck="false" />
        <span class="hint">{{ $t("nextcloud.ncUserHint") }}</span>
      </label>
      <label class="field">
        <span class="label-line">{{ $t("nextcloud.host") }}<span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('nextcloud.help.host')" example="proxmox.local · nextcloud.tailnet" :label="$t('nextcloud.host')" />
        </span>
        <input v-model="form.host" required spellcheck="false" placeholder="proxmox.local" />
        <span class="hint">{{ $t("nextcloud.hint.host") }}</span>
      </label>
      <label class="field narrow">
        <span class="label-line">{{ $t("profile.port") }}
          <HelpTip :text="$t('profile.help.port')" example="22" :label="$t('profile.port')" />
        </span>
        <input v-model.number="form.port" type="number" min="1" max="65535" />
      </label>
      <label class="field">
        <span class="label-line">{{ $t("nextcloud.sshUser") }}<span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('nextcloud.help.sshUser')" example="root" :label="$t('nextcloud.sshUser')" />
        </span>
        <input v-model="form.user" required spellcheck="false" />
        <span class="hint">{{ $t("nextcloud.hint.sshUser") }}</span>
      </label>
      <label class="field">
        <span class="label-line">{{ $t("nextcloud.folder") }}
          <HelpTip :text="$t('nextcloud.help.folder')" example="Backup/2026" :label="$t('nextcloud.folder')" />
        </span>
        <input v-model="form.folder" spellcheck="false" :placeholder="$t('common.optional')" />
        <span v-if="folderLooksLikeHostPath" class="small warn-text">{{ $t("nextcloud.folderHostPath") }}</span>
        <span v-else class="hint">{{ $t("nextcloud.folderHint") }}</span>
      </label>
      <label class="field wide">
        <span class="label-line">{{ $t("profile.keyFile") }}
          <HelpTip :text="$t('profile.help.keyFile')" example="~/.ssh/id_ed25519" :label="$t('profile.keyFile')" />
        </span>
        <span class="with-button">
          <input v-model="form.key_file" spellcheck="false" :placeholder="$t('profile.keyFileHint')" />
          <button type="button" class="btn btn-small" @click="pickKey">{{ $t("common.browse") }}</button>
        </span>
        <span class="hint">{{ $t("profile.hint.keyFile") }}</span>
      </label>
    </div>

    <button type="button" class="link small" :aria-expanded="advanced" @click="advanced = !advanced">
      <AppIcon :name="advanced ? 'caretDown' : 'caretRight'" /> {{ $t("common.advanced") }}
    </button>
    <span v-if="!advanced" class="small muted adv-hint">{{ $t("nextcloud.advancedHint") }}</span>
    <div v-if="advanced" class="grid">
      <label class="field">
        <span class="label-line">{{ $t("profile.knownHosts") }}
          <HelpTip :text="$t('profile.help.knownHosts')" example="~/.ssh/known_hosts" :label="$t('profile.knownHosts')" />
        </span>
        <input v-model="form.known_hosts_file" spellcheck="false" placeholder="~/.ssh/known_hosts" />
      </label>
      <label class="field narrow">
        <span class="label-line">{{ $t("nextcloud.ctid") }}
          <HelpTip :text="$t('nextcloud.help.ctid')" example="105" :label="$t('nextcloud.ctid')" />
        </span>
        <input v-model="form.ctid" inputmode="numeric" :placeholder="$t('nextcloud.auto')" />
      </label>
      <label class="field">
        <span class="label-line">{{ $t("nextcloud.installDir") }}
          <HelpTip :text="$t('nextcloud.help.installDir')" example="/var/www/nextcloud" :label="$t('nextcloud.installDir')" />
        </span>
        <input v-model="form.install_dir" spellcheck="false" :placeholder="$t('nextcloud.auto')" />
      </label>
    </div>

    <div class="actions">
      <span v-if="missing.length" class="why-disabled">{{ $t("profile.missing", { fields: missing.join(", ") }) }}</span>
      <span class="grow" />
      <button type="button" class="btn" @click="emit('cancel')">{{ $t("common.cancel") }}</button>
      <button type="submit" class="btn btn-primary" :title="$t('nextcloud.detectTip')" :disabled="!valid || running || !!result?.profile">
        {{ running ? $t("nextcloud.detecting") : $t("nextcloud.detect") }}
      </button>
    </div>

    <p v-if="error" class="error small">{{ error }}</p>

    <div v-if="result" class="result">
      <ol class="steps">
        <li v-for="s in result.steps" :key="s.id">
          <StatusIcon :status="s.status" />
          <span><strong>{{ stepLabel(s.id) }}</strong> <span class="muted">{{ s.message }}</span></span>
        </li>
      </ol>
      <div v-if="result.containers.length" class="containers">
        <p class="small">{{ $t("nextcloud.pickContainer") }}</p>
        <button v-for="c in result.containers" :key="c" type="button" class="btn btn-small" @click="chooseContainer(c)">{{ c }}</button>
      </div>
      <p v-if="result.profile && te(runtimeKey)" class="small muted">{{ t(runtimeKey) }}</p>
      <div v-if="result.profile" class="saved">
        <p class="ok-text small">{{ $t("nextcloud.saved", { name: result.profile.name }) }}</p>
        <button type="button" class="btn btn-primary" @click="emit('saved', result.profile!)">{{ $t("nextcloud.done") }}</button>
      </div>
      <DetailsPanel :lines="commands" />
    </div>
  </form>
</template>

<style scoped>
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 0.75rem; align-items: start; }
.wide { grid-column: 1 / -1; }
.with-button { display: flex; gap: 0.5rem; }
.with-button input { flex: 1; }
.actions { display: flex; justify-content: flex-end; align-items: center; gap: 0.5rem; margin-top: 1rem; flex-wrap: wrap; }
.adv-hint { margin-left: 0.5rem; }
.steps { list-style: none; padding: 0; margin: 1rem 0 0; }
.steps li { display: flex; gap: 0.5rem; align-items: flex-start; margin: 0.45rem 0; }
.saved { display: flex; gap: 1rem; align-items: center; justify-content: space-between; }
.containers { display: flex; gap: 0.5rem; flex-wrap: wrap; align-items: center; }
.link { all: unset; cursor: pointer; color: var(--accent); margin: 0.75rem 0 0.5rem; display: inline-block; }
</style>
