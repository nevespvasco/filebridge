<script setup lang="ts">
// Nextcloud direct mode (docs/05): SSH to the Proxmox host, find the Nextcloud container, its
// data directory, `occ` and the file owner. Each step shows ✓ / ! / ✗, and the Details panel the
// commands it ran.
import { computed, reactive, ref, shallowRef } from "vue";
import { useI18n } from "vue-i18n";
import type { DetectNextcloudResponse_Serialize, Profile_Serialize } from "@/bindings";
import { errorMessage } from "@/lib/api";
import { pickPaths } from "@/lib/platform";
import { useProfiles } from "@/stores/profiles";
import StatusIcon from "./StatusIcon.vue";
import DetailsPanel from "./DetailsPanel.vue";

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

const valid = computed(() => form.name.trim() && form.host.trim() && form.user.trim() && form.nextcloud_user.trim() && form.port > 0);
const commands = computed(() => result.value?.steps.flatMap((s) => [`# ${s.id}`, ...s.commands]) ?? []);

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
      folder: form.folder.trim().replace(/^\/+|\/+$/g, ""),
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
        <span>{{ $t("profile.name") }}</span>
        <input v-model="form.name" required />
      </label>
      <label class="field">
        <span>{{ $t("nextcloud.ncUser") }}</span>
        <input v-model="form.nextcloud_user" required spellcheck="false" :placeholder="$t('nextcloud.ncUserHint')" />
      </label>
      <label class="field">
        <span>{{ $t("nextcloud.host") }}</span>
        <input v-model="form.host" required spellcheck="false" placeholder="pve.example.lan" />
      </label>
      <label class="field narrow">
        <span>{{ $t("profile.port") }}</span>
        <input v-model.number="form.port" type="number" min="1" max="65535" />
      </label>
      <label class="field">
        <span>{{ $t("nextcloud.sshUser") }}</span>
        <input v-model="form.user" required spellcheck="false" />
      </label>
      <label class="field">
        <span>{{ $t("nextcloud.folder") }}</span>
        <input v-model="form.folder" spellcheck="false" :placeholder="$t('nextcloud.folderHint')" />
      </label>
      <label class="field wide">
        <span>{{ $t("profile.keyFile") }}</span>
        <span class="with-button">
          <input v-model="form.key_file" spellcheck="false" :placeholder="$t('profile.keyFileHint')" />
          <button type="button" class="btn btn-small" @click="pickKey">{{ $t("common.browse") }}</button>
        </span>
      </label>
    </div>

    <button type="button" class="link small" :aria-expanded="advanced" @click="advanced = !advanced">
      {{ advanced ? "▾" : "▸" }} {{ $t("common.advanced") }}
    </button>
    <div v-if="advanced" class="grid">
      <label class="field">
        <span>{{ $t("profile.knownHosts") }}</span>
        <input v-model="form.known_hosts_file" spellcheck="false" placeholder="~/.ssh/known_hosts" />
      </label>
      <label class="field narrow">
        <span>{{ $t("nextcloud.ctid") }}</span>
        <input v-model="form.ctid" inputmode="numeric" :placeholder="$t('nextcloud.auto')" />
      </label>
      <label class="field">
        <span>{{ $t("nextcloud.installDir") }}</span>
        <input v-model="form.install_dir" spellcheck="false" :placeholder="$t('nextcloud.auto')" />
      </label>
    </div>

    <div class="actions">
      <button type="button" class="btn" @click="emit('cancel')">{{ $t("common.cancel") }}</button>
      <button type="submit" class="btn btn-primary" :disabled="!valid || running || !!result?.profile">
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
      <div v-if="result.profile" class="saved">
        <p class="ok-text small">{{ $t("nextcloud.saved", { name: result.profile.name }) }}</p>
        <button type="button" class="btn btn-primary" @click="emit('saved', result.profile!)">{{ $t("nextcloud.done") }}</button>
      </div>
      <DetailsPanel :lines="commands" />
    </div>
  </form>
</template>

<style scoped>
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 0.75rem; }
.wide { grid-column: 1 / -1; }
.with-button { display: flex; gap: 0.5rem; }
.with-button input { flex: 1; }
.actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 1rem; }
.steps { list-style: none; padding: 0; margin: 1rem 0 0; }
.steps li { display: flex; gap: 0.5rem; align-items: flex-start; margin: 0.45rem 0; }
.saved { display: flex; gap: 1rem; align-items: center; justify-content: space-between; }
.containers { display: flex; gap: 0.5rem; flex-wrap: wrap; align-items: center; }
.link { all: unset; cursor: pointer; color: var(--accent); margin: 0.75rem 0 0.5rem; display: inline-block; }
</style>
