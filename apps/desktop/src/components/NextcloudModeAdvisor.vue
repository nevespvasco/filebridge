<script setup lang="ts">
// "Which mode?" (ADR 0024): filebridge looks at the server over SSH, as direct mode's detection
// does but saving nothing, and recommends direct mode when SSH reaches the machine Nextcloud runs
// on with the rights it needs, WebDAV otherwise, saying why. The user can still pick either.
import { computed, reactive, ref, shallowRef } from "vue";
import { useI18n } from "vue-i18n";
import type { NextcloudAdvice } from "@/bindings";
import { errorMessage } from "@/lib/api";
import { useProfiles } from "@/stores/profiles";
import AppIcon from "./AppIcon.vue";
import HelpTip from "./HelpTip.vue";
import HostKeyPrompt from "./HostKeyPrompt.vue";

export type AdvisedMode = "webdav" | "direct";
/** What the user typed here, to fill in the chosen mode's form. */
export interface AdvisorFields {
  host: string;
  port: number;
  user: string;
  nextcloud_user: string;
}

const emit = defineEmits<{ advised: [mode: AdvisedMode, fields: AdvisorFields] }>();
const { t } = useI18n();
const profiles = useProfiles();

const open = ref(false);
const form = reactive<AdvisorFields>({ host: "", port: 22, user: "root", nextcloud_user: "" });
const running = ref(false);
const error = ref<string | null>(null);
/** The service's advice, or the window's own when the user didn't trust the server's key. */
const advice = shallowRef<Pick<NextcloudAdvice, "mode" | "reason" | "message" | "steps"> | null>(null);
const valid = computed(() => !!form.host.trim() && form.port > 0 && !!form.user.trim() && !!form.nextcloud_user.trim());
const modeLabel = computed(() => (advice.value?.mode === "direct" ? t("webdav.modeDirect") : t("webdav.modeWebdav")));

const hostKey = ref<InstanceType<typeof HostKeyPrompt> | null>(null);

async function check() {
  running.value = true;
  error.value = null;
  advice.value = null;
  const fields = { host: form.host.trim(), port: form.port, user: form.user.trim(), nextcloud_user: form.nextcloud_user.trim() };
  try {
    let known: boolean;
    try {
      known = await hostKey.value!.ensureKnown(fields.host, fields.port, null);
    } catch (e) {
      // Not even the SSH port answers: direct mode can't work from here.
      advice.value = { mode: "webdav", reason: "unreachable", message: errorMessage(e), steps: [] };
      emit("advised", "webdav", fields);
      return;
    }
    if (!known) {
      advice.value = { mode: "webdav", reason: "untrusted", message: "", steps: [] };
      emit("advised", "webdav", fields);
      return;
    }
    const res = await profiles.probeNextcloud({
      name: "",
      host: fields.host,
      port: fields.port,
      user: fields.user,
      key_file: null,
      known_hosts_file: null,
      nextcloud_user: fields.nextcloud_user,
      folder: "",
      ctid: null,
      install_dir: null,
    });
    advice.value = res;
    emit("advised", res.mode === "direct" ? "direct" : "webdav", fields);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    running.value = false;
  }
}
</script>

<template>
  <div class="advisor">
    <button type="button" class="link small" :aria-expanded="open" @click="open = !open">
      <AppIcon :name="open ? 'caretDown' : 'caretRight'" /> {{ $t("nextcloud.advice.title") }}
    </button>
    <form v-if="open" class="box" @submit.prevent="check">
      <p class="small muted">{{ $t("nextcloud.advice.intro") }}</p>
      <div class="grid">
        <label class="field">
          <span class="label-line">{{ $t("nextcloud.host") }}
            <HelpTip :text="$t('nextcloud.help.host')" example="proxmox.local · nextcloud.tailnet" :label="$t('nextcloud.host')" />
          </span>
          <input v-model="form.host" required spellcheck="false" placeholder="nextcloud.local" />
        </label>
        <label class="field narrow">
          <span class="label-line">{{ $t("profile.port") }}</span>
          <input v-model.number="form.port" type="number" min="1" max="65535" />
        </label>
        <label class="field">
          <span class="label-line">{{ $t("nextcloud.sshUser") }}</span>
          <input v-model="form.user" required spellcheck="false" />
        </label>
        <label class="field">
          <span class="label-line">{{ $t("nextcloud.ncUser") }}</span>
          <input v-model="form.nextcloud_user" required spellcheck="false" />
        </label>
      </div>
      <div class="actions">
        <button type="submit" class="btn btn-small" :disabled="!valid || running">
          {{ running ? $t("nextcloud.advice.checking") : $t("nextcloud.advice.check") }}
        </button>
      </div>
      <p v-if="error" class="error small">{{ error }}</p>
      <div v-if="advice" class="result" role="status">
        <p class="small"><strong>{{ $t("nextcloud.advice.recommended", { mode: modeLabel }) }}</strong></p>
        <p class="small">{{ $t(`nextcloud.advice.${advice.reason}`) }}</p>
        <p v-if="advice.message" class="small muted">{{ advice.message }}</p>
        <p class="small muted">{{ $t("nextcloud.advice.override") }}</p>
      </div>
      <HostKeyPrompt ref="hostKey" />
    </form>
  </div>
</template>

<style scoped>
.advisor { margin: 0 0 0.75rem; }
.box { border: 1px solid var(--border); border-radius: var(--radius-md); padding: 0.75rem; margin-top: 0.25rem; }
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: 0.75rem; align-items: start; }
.actions { display: flex; justify-content: flex-end; margin-top: 0.75rem; }
.result { margin-top: 0.5rem; }
.result p { margin: 0.25rem 0; }
.link { all: unset; cursor: pointer; color: var(--accent); }
</style>
