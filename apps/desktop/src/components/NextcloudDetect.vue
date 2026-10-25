<script setup lang="ts">
// Nextcloud direct mode (docs/05): SSH to the Proxmox host (and find the Nextcloud container) or
// to the machine Nextcloud runs on, then find its data directory, `occ` and the file owner. Each
// step shows ✓ / ! / ✗.
import { trimBoth } from "@/lib/format";
import AppIcon from "@/components/AppIcon.vue";
import { computed, reactive, ref, shallowRef, useId } from "vue";
import { useI18n } from "vue-i18n";
import type { DetectNextcloudResponse_Serialize, Profile_Serialize } from "@/bindings";
import { errorMessage } from "@/lib/api";
import { copyText, pickPaths } from "@/lib/platform";
import { useProfiles } from "@/stores/profiles";
import StatusIcon from "./StatusIcon.vue";
import HelpTip from "./HelpTip.vue";
import HostKeyPrompt from "./HostKeyPrompt.vue";
import type { AdvisorFields } from "./NextcloudModeAdvisor.vue";

/** What the mode advisor was given, so it isn't typed twice. */
const props = defineProps<{ initial?: AdvisorFields | null }>();
const emit = defineEmits<{ saved: [profile: Profile_Serialize]; cancel: [] }>();
const { t, te } = useI18n();
const profiles = useProfiles();

const form = reactive({
  name: "Nextcloud",
  host: props.initial?.host ?? "",
  port: props.initial?.port ?? 22,
  user: props.initial?.user ?? "root",
  key_file: "",
  known_hosts_file: "",
  nextcloud_user: props.initial?.nextcloud_user ?? "",
  folder: "",
  ctid: "",
  install_dir: "",
  dedicated_user: false,
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
// The folder is relative to the Nextcloud user's files; an absolute path under a usual server
// mount point (e.g. /mnt/pve/data/…) is almost certainly the data folder on the host instead.
const folderLooksLikeHostPath = computed(() =>
  /^\/(mnt|srv|var|home|media|opt|data|tank|rpool|storage|root|usr|etc)(\/|$)/i.test(form.folder.trim()),
);
/** The dedicated user's server setup (ADR 0024), for the user to read and run themselves. */
const setupText = computed(() => result.value?.server_setup.join("\n") ?? "");
const copied = ref(false);
async function copySetup() {
  try {
    await copyText(setupText.value);
    copied.value = true;
    setTimeout(() => (copied.value = false), 2000);
  } catch {
    copied.value = false;
  }
}
const runtimeKey = computed(() => `nextcloud.runtime.${result.value?.detector ?? ""}`);

function stepLabel(id: string) {
  const key = `nextcloud.steps.${id}`;
  return te(key) ? t(key) : id;
}

async function pickKey() {
  const [p] = await pickPaths({ directory: false, multiple: false });
  if (p) form.key_file = p;
}

/** The SSH server's host key, trusted by the user if known_hosts doesn't have it (ADR 0015). */
const hostKey = ref<InstanceType<typeof HostKeyPrompt> | null>(null);

async function detect() {
  running.value = true;
  error.value = null;
  result.value = null;
  try {
    const host = form.host.trim();
    let known: boolean;
    try {
      known = await hostKey.value!.ensureKnown(host, form.port, form.known_hosts_file.trim() || null);
    } catch (e) {
      error.value = t("hostKey.unreachable", { error: errorMessage(e) });
      return;
    }
    if (!known) return;
    const ctid = form.ctid.trim() ? Number(form.ctid) : null;
    result.value = await profiles.detectNextcloud({
      name: form.name.trim(),
      host,
      port: form.port,
      user: form.user.trim(),
      key_file: form.key_file.trim() || null,
      known_hosts_file: form.known_hosts_file.trim() || null,
      nextcloud_user: form.nextcloud_user.trim(),
      folder: trimBoth(form.folder.trim(), "/"),
      ctid: ctid != null && Number.isInteger(ctid) ? ctid : null,
      install_dir: form.install_dir.trim() || null,
      dedicated_user: form.dedicated_user,
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
const uid = useId();
</script>

<template>
  <form class="nc" @submit.prevent="detect">
    <p class="small muted">{{ $t("nextcloud.intro") }}</p>
    <div class="form-grid">
      <div class="field">
        <span class="label-line"><label :for="`${uid}-1`">{{ $t("profile.name") }}</label><span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('profile.help.name')" :label="$t('profile.name')" />
        </span>
        <input :id="`${uid}-1`" v-model="form.name" required />
      </div>
      <div class="field">
        <span class="label-line"><label :for="`${uid}-2`">{{ $t("nextcloud.ncUser") }}</label><span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('nextcloud.help.ncUser')" example="maria" :label="$t('nextcloud.ncUser')" />
        </span>
        <input :id="`${uid}-2`" v-model="form.nextcloud_user" required spellcheck="false" />
        <span class="hint">{{ $t("nextcloud.ncUserHint") }}</span>
      </div>
      <div class="field">
        <span class="label-line"><label :for="`${uid}-3`">{{ $t("nextcloud.host") }}</label><span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('nextcloud.help.host')" example="proxmox.local · nextcloud.tailnet" :label="$t('nextcloud.host')" />
        </span>
        <input :id="`${uid}-3`" v-model="form.host" required spellcheck="false" placeholder="proxmox.local" />
        <span class="hint">{{ $t("nextcloud.hint.host") }}</span>
      </div>
      <div class="field narrow">
        <span class="label-line"><label :for="`${uid}-4`">{{ $t("profile.port") }}</label>
          <HelpTip :text="$t('profile.help.port')" example="22" :label="$t('profile.port')" />
        </span>
        <input :id="`${uid}-4`" v-model.number="form.port" type="number" min="1" max="65535" />
      </div>
      <div class="field">
        <span class="label-line"><label :for="`${uid}-5`">{{ $t("nextcloud.sshUser") }}</label><span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('nextcloud.help.sshUser')" example="root" :label="$t('nextcloud.sshUser')" />
        </span>
        <input :id="`${uid}-5`" v-model="form.user" required spellcheck="false" />
        <span class="hint">{{ $t("nextcloud.hint.sshUser") }}</span>
      </div>
      <div class="field">
        <span class="label-line"><label :for="`${uid}-6`">{{ $t("nextcloud.folder") }}</label>
          <HelpTip :text="$t('nextcloud.help.folder')" example="Backup/2026" :label="$t('nextcloud.folder')" />
        </span>
        <input :id="`${uid}-6`" v-model="form.folder" spellcheck="false" :placeholder="$t('common.optional')" />
        <span v-if="folderLooksLikeHostPath" class="small warn-text">{{ $t("nextcloud.folderHostPath") }}</span>
        <span v-else class="hint">{{ $t("nextcloud.folderHint") }}</span>
      </div>
      <div class="field wide">
        <span class="label-line"><label :for="`${uid}-7`">{{ $t("profile.keyFile") }}</label>
          <HelpTip :text="$t('profile.help.keyFile')" example="~/.ssh/id_ed25519" :label="$t('profile.keyFile')" />
        </span>
        <span class="with-button">
          <input :id="`${uid}-7`" v-model="form.key_file" spellcheck="false" :placeholder="$t('profile.keyFileHint')" />
          <button type="button" class="btn btn-small" @click="pickKey">{{ $t("common.browse") }}</button>
        </span>
        <span class="hint">{{ $t("profile.hint.keyFile") }}</span>
      </div>
    </div>

    <button type="button" class="link small" :aria-expanded="advanced" @click="advanced = !advanced">
      <AppIcon :name="advanced ? 'caretDown' : 'caretRight'" /> {{ $t("common.advanced") }}
    </button>
    <span v-if="!advanced" class="small muted adv-hint">{{ $t("nextcloud.advancedHint") }}</span>
    <div v-if="advanced" class="form-grid">
      <div class="field">
        <span class="label-line"><label :for="`${uid}-8`">{{ $t("profile.knownHosts") }}</label>
          <HelpTip :text="$t('profile.help.knownHosts')" example="~/.ssh/known_hosts" :label="$t('profile.knownHosts')" />
        </span>
        <input :id="`${uid}-8`" v-model="form.known_hosts_file" spellcheck="false" placeholder="~/.ssh/known_hosts" />
      </div>
      <div class="field narrow">
        <span class="label-line"><label :for="`${uid}-9`">{{ $t("nextcloud.ctid") }}</label>
          <HelpTip :text="$t('nextcloud.help.ctid')" example="105" :label="$t('nextcloud.ctid')" />
        </span>
        <input :id="`${uid}-9`" v-model="form.ctid" inputmode="numeric" :placeholder="$t('nextcloud.auto')" />
      </div>
      <div class="field">
        <span class="label-line"><label :for="`${uid}-10`">{{ $t("nextcloud.installDir") }}</label>
          <HelpTip :text="$t('nextcloud.help.installDir')" example="/var/www/nextcloud" :label="$t('nextcloud.installDir')" />
        </span>
        <input :id="`${uid}-10`" v-model="form.install_dir" spellcheck="false" :placeholder="$t('nextcloud.auto')" />
      </div>
      <div class="field wide check">
        <span class="label-line">
          <input :id="`${uid}-11`" v-model="form.dedicated_user" type="checkbox" />
          <label :for="`${uid}-11`">{{ $t("nextcloud.dedicated") }}</label>
          <HelpTip :text="$t('nextcloud.help.dedicated')" :label="$t('nextcloud.dedicated')" />
        </span>
        <span v-if="form.dedicated_user" class="hint">{{ $t("nextcloud.dedicatedHint") }}</span>
      </div>
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
      <div v-if="setupText" class="server-setup">
        <p class="small"><strong>{{ $t("nextcloud.serverSetup") }}</strong></p>
        <p class="small muted">{{ $t("nextcloud.serverSetupHint") }}</p>
        <pre><code>{{ setupText }}</code></pre>
        <button type="button" class="btn btn-small" @click="copySetup">{{ copied ? $t("nextcloud.copied") : $t("nextcloud.copySetup") }}</button>
      </div>
      <p v-if="result.profile && te(runtimeKey)" class="small muted">{{ t(runtimeKey) }}</p>
      <div v-if="result.profile" class="saved">
        <p class="ok-text small">{{ $t("nextcloud.saved", { name: result.profile.name }) }}</p>
        <button type="button" class="btn btn-primary" @click="emit('saved', result.profile!)">{{ $t("nextcloud.done") }}</button>
      </div>
    </div>
    <HostKeyPrompt ref="hostKey" />
  </form>
</template>

<style scoped>
.with-button { display: flex; gap: 0.5rem; }
.with-button input { flex: 1; }
.actions { display: flex; justify-content: flex-end; align-items: center; gap: 0.5rem; margin-top: 1rem; flex-wrap: wrap; }
.adv-hint { margin-left: 0.5rem; }
.steps { list-style: none; padding: 0; margin: 1rem 0 0; }
.steps li { display: flex; gap: 0.5rem; align-items: flex-start; margin: 0.45rem 0; }
.saved { display: flex; gap: 1rem; align-items: center; justify-content: space-between; }
.containers { display: flex; gap: 0.5rem; flex-wrap: wrap; align-items: center; }
.check .label-line { display: flex; gap: 0.4rem; align-items: center; }
.server-setup { margin-top: 0.75rem; }
.server-setup pre { overflow-x: auto; font-size: 0.8rem; line-height: 1.5; background: var(--surface-2); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 0.5rem 0.75rem; }
.link { all: unset; cursor: pointer; color: var(--accent); margin: 0.75rem 0 0.5rem; display: inline-block; }
</style>
