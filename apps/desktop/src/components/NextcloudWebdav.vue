<script setup lang="ts">
// Nextcloud in WebDAV mode (docs/05): its address, the user and an app password. The password
// goes to the system keyring when the profile is saved, and never into the profile, the logs or
// the Details panel. Each check shows ✓ / ! / ✗. A server at home may be reached by http://, or
// with a certificate nothing vouches for, when the user chooses so (ADR 0035).
import { computed, reactive, ref, shallowRef, useId, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { DetectNextcloudResponse_Serialize, Profile_Serialize } from "@/bindings";
import { errorMessage } from "@/lib/api";
import { openInBrowser } from "@/lib/platform";
import { trimBoth } from "@/lib/format";
import { useProfiles } from "@/stores/profiles";
import AppIcon from "./AppIcon.vue";
import HelpTip from "./HelpTip.vue";
import StatusIcon from "./StatusIcon.vue";

const props = defineProps<{ /** The Nextcloud user typed in the mode advisor. */ initialUser?: string }>();
const emit = defineEmits<{ saved: [profile: Profile_Serialize]; cancel: [] }>();
const { t, te } = useI18n();
const profiles = useProfiles();

const form = reactive({ name: "Nextcloud", server: "", user: props.initialUser ?? "", folder: "" });
/** Kept apart from the form and cleared once used. */
const appPassword = ref("");
const running = ref(false);
const error = ref<string | null>(null);
const result = shallowRef<DetectNextcloudResponse_Serialize | null>(null);
/** The user's leave for http:// to a server at home; the service checks it is at home. */
const allowHttp = ref(false);
/** The user trusts the server's certificate, whatever it is. */
const trustCertificate = ref(false);
/** The connection options: open or closed as the user left them, opened again by an IP
 *  address or a refused certificate. The one state the disclosure follows, so closing it by
 *  hand never keeps it from opening later. */
const connectionOpen = ref(false);

const missing = computed(() => {
  const m: string[] = [];
  if (!form.name.trim()) m.push(t("profile.name"));
  if (!form.server.trim()) m.push(t("webdav.server"));
  if (!form.user.trim()) m.push(t("nextcloud.ncUser"));
  if (!appPassword.value) m.push(t("webdav.appPassword"));
  return m;
});
/** http:// is refused by the service unless allowed; say so before asking it. */
const plainHttp = computed(() => /^http:\/\//i.test(form.server.trim()) && !/^http:\/\/(localhost|127\.|\[::1\])/i.test(form.server.trim()));
/** An IP address over HTTPS: the certificate names the server, so the check would fail. */
const ipAddress = computed(() => /^(https:\/\/)?(\d{1,3}(\.\d{1,3}){3}|\[[0-9a-f:]+\])(:\d+)?(\/|$)/i.test(form.server.trim()));
watch(ipAddress, (ip) => {
  if (ip) connectionOpen.value = true;
});
/** Nextcloud's app passwords are 29 characters (xxxxx-xxxxx-…) or 72 (login flow): anything
 *  much shorter is the account password. Said, not refused. */
/** The sign-in failed on the server's certificate, which trusting it gets past. */
const certificateRefused = computed(() => !!result.value?.steps.some((s) => s.hint === "certificate"));
const shortPassword = computed(() => appPassword.value.trim().length > 0 && appPassword.value.trim().length < 25);

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
      plain_http: plainHttp.value && allowHttp.value,
      skip_certificate_check: !plainHttp.value && trustCertificate.value,
    });
    if (result.value.profile) appPassword.value = "";
    if (certificateRefused.value) connectionOpen.value = true;
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
        <template v-if="plainHttp">
          <label class="check">
            <input v-model="allowHttp" type="checkbox" />
            <span>{{ $t("webdav.plainHttp") }}</span>
          </label>
          <span class="hint warn-text">{{ allowHttp ? $t("webdav.plainHttpWarn") : $t("webdav.https") }}</span>
        </template>
        <template v-else>
          <span v-if="ipAddress" class="hint warn-text">{{ $t("webdav.ipAddress") }}</span>
          <details class="connection" :open="connectionOpen" @toggle="connectionOpen = ($event.target as HTMLDetailsElement).open">
            <summary class="small">{{ $t("webdav.connectionOptions") }}</summary>
            <label class="check">
              <input v-model="trustCertificate" type="checkbox" />
              <span>{{ $t("webdav.trustCertificate") }}</span>
            </label>
            <span class="hint" :class="{ 'warn-text': trustCertificate }">{{ $t("webdav.trustCertificateHint") }}</span>
          </details>
        </template>
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
        <span v-if="shortPassword" class="hint warn-text">{{ $t("webdav.appPasswordShort") }}</span>
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
      <p v-if="certificateRefused && !trustCertificate" class="hint small warn-text">{{ $t("webdav.trustCertificateAgain") }}</p>
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
/* Messages carry addresses with no spaces: they wrap anywhere rather than widen the page. */
.steps li > span { min-width: 0; overflow-wrap: anywhere; }
.check { display: flex; gap: 0.5rem; align-items: flex-start; margin-top: 0.4rem; cursor: pointer; }
.check input { margin-top: 0.2rem; }
.connection { margin-top: 0.4rem; }
.connection summary { cursor: pointer; color: var(--muted); }
.saved { display: flex; gap: 1rem; align-items: center; justify-content: space-between; }
</style>
