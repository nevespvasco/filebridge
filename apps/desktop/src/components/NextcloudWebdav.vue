<script setup lang="ts">
// Nextcloud in WebDAV mode (docs/05): its address, the user and an app password. The password
// goes to the system keyring when the profile is saved, and never into the profile, the logs or
// the Details panel. Each check shows ✓ / ! / ✗.
import { computed, reactive, ref, shallowRef, useId } from "vue";
import { useI18n } from "vue-i18n";
import type { DetectNextcloudResponse_Serialize, Profile_Serialize } from "@/bindings";
import { errorMessage } from "@/lib/api";
import { openInBrowser } from "@/lib/platform";
import { trimBoth } from "@/lib/format";
import { useProfiles } from "@/stores/profiles";
import AppIcon from "./AppIcon.vue";
import HelpTip from "./HelpTip.vue";
import StatusIcon from "./StatusIcon.vue";

const emit = defineEmits<{ saved: [profile: Profile_Serialize]; cancel: [] }>();
const { t, te } = useI18n();
const profiles = useProfiles();

const form = reactive({ name: "Nextcloud", server: "", user: "", folder: "" });
/** Kept apart from the form and cleared once used. */
const appPassword = ref("");
const running = ref(false);
const error = ref<string | null>(null);
const result = shallowRef<DetectNextcloudResponse_Serialize | null>(null);

const missing = computed(() => {
  const m: string[] = [];
  if (!form.name.trim()) m.push(t("profile.name"));
  if (!form.server.trim()) m.push(t("webdav.server"));
  if (!form.user.trim()) m.push(t("nextcloud.ncUser"));
  if (!appPassword.value) m.push(t("webdav.appPassword"));
  return m;
});
/** http:// is refused by the service; say so before asking it. */
const plainHttp = computed(() => /^http:\/\//i.test(form.server.trim()) && !/^http:\/\/(localhost|127\.|\[::1\])/i.test(form.server.trim()));

/** The server's Security settings, where app passwords are made, once the address looks like one. */
const securityPage = computed(() => {
  let s = form.server.trim();
  if (!s) return null;
  if (!/^[a-z]+:\/\//i.test(s)) s = `https://${s}`;
  try {
    const url = new URL(s.replace(/\/remote\.php(\/.*)?$/i, ""));
    if (url.protocol !== "https:" || !url.hostname.includes(".")) return null;
    return `${url.origin}${url.pathname.replace(/\/+$/, "")}/index.php/settings/user/security`;
  } catch {
    return null;
  }
});

function stepLabel(id: string) {
  const key = `webdav.steps.${id}`;
  return te(key) ? t(key) : id;
}

async function check() {
  running.value = true;
  error.value = null;
  result.value = null;
  try {
    result.value = await profiles.addNextcloudWebdav({
      name: form.name.trim(),
      server: form.server.trim(),
      user: form.user.trim(),
      app_password: appPassword.value,
      folder: trimBoth(form.folder.trim(), "/"),
    });
    if (result.value.profile) appPassword.value = "";
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    running.value = false;
  }
}
const uid = useId();
</script>

<template>
  <form class="nc" @submit.prevent="check">
    <p class="small muted">{{ $t("webdav.intro") }}</p>
    <div class="form-grid">
      <div class="field">
        <span class="label-line"><label :for="`${uid}-1`">{{ $t("profile.name") }}</label><span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('profile.help.name')" :label="$t('profile.name')" />
        </span>
        <input :id="`${uid}-1`" v-model="form.name" required />
      </div>
      <div class="field">
        <span class="label-line"><label :for="`${uid}-2`">{{ $t("webdav.server") }}</label><span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('webdav.help.server')" example="cloud.example.com" :label="$t('webdav.server')" />
        </span>
        <input :id="`${uid}-2`" v-model="form.server" spellcheck="false" autocomplete="url" placeholder="cloud.example.com" />
        <span v-if="plainHttp" class="hint warn-text">{{ $t("webdav.https") }}</span>
      </div>
      <div class="field">
        <span class="label-line"><label :for="`${uid}-3`">{{ $t("nextcloud.ncUser") }}</label><span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('nextcloud.help.ncUser')" example="maria" :label="$t('nextcloud.ncUser')" />
        </span>
        <input :id="`${uid}-3`" v-model="form.user" spellcheck="false" autocomplete="username" />
      </div>
      <div class="field">
        <span class="label-line"><label :for="`${uid}-4`">{{ $t("webdav.appPassword") }}</label><span class="req" aria-hidden="true">*</span>
          <HelpTip :text="$t('webdav.help.appPassword')" :label="$t('webdav.appPassword')" />
        </span>
        <input :id="`${uid}-4`" v-model="appPassword" type="password" autocomplete="new-password" spellcheck="false" />
        <span class="hint">
          {{ $t("webdav.appPasswordHint") }}
          <a v-if="securityPage" :href="securityPage" @click.prevent="openInBrowser(securityPage)">{{ $t("webdav.appPasswordLink") }}</a>
        </span>
        <span class="hint"><AppIcon name="lock" :size="16" /> {{ $t("profile.hint.password") }}</span>
      </div>
      <div class="field wide">
        <span class="label-line"><label :for="`${uid}-5`">{{ $t("nextcloud.folder") }}</label>
          <HelpTip :text="$t('webdav.help.folder')" example="Backup/Fotos" :label="$t('nextcloud.folder')" />
        </span>
        <input :id="`${uid}-5`" v-model="form.folder" spellcheck="false" :placeholder="$t('webdav.folderHint')" />
      </div>
    </div>

    <div class="actions">
      <span v-if="missing.length && !result?.profile" class="why-disabled">{{ $t("profile.missing", { fields: missing.join(", ") }) }}</span>
      <span class="grow" />
      <button type="button" class="btn" @click="emit('cancel')">{{ $t("common.cancel") }}</button>
      <button type="submit" class="btn btn-primary" :disabled="!!missing.length || running || !!result?.profile">
        {{ running ? $t("webdav.checking") : $t("webdav.check") }}
      </button>
    </div>

    <p v-if="error" class="error small" role="alert">{{ error }}</p>

    <div v-if="result" class="result" aria-live="polite">
      <ol class="steps">
        <li v-for="s in result.steps" :key="s.id">
          <StatusIcon :status="s.status" />
          <span><strong>{{ stepLabel(s.id) }}</strong> <span class="muted">{{ s.message }}</span></span>
        </li>
      </ol>
      <div v-if="result.profile" class="saved">
        <p class="ok-text small">{{ $t("nextcloud.saved", { name: result.profile.name }) }}</p>
        <button type="button" class="btn btn-primary" @click="emit('saved', result.profile!)">{{ $t("nextcloud.done") }}</button>
      </div>
    </div>
  </form>
</template>

<style scoped>
.actions { display: flex; justify-content: flex-end; align-items: center; gap: 0.5rem; margin-top: 1rem; flex-wrap: wrap; }
.grow { flex: 1; }
.steps { list-style: none; padding: 0; margin: 1rem 0 0; }
.steps li { display: flex; gap: 0.5rem; align-items: flex-start; margin: 0.45rem 0; }
.saved { display: flex; gap: 1rem; align-items: center; justify-content: space-between; }
</style>
