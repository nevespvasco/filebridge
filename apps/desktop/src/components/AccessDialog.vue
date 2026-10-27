<script setup lang="ts">
// filebridge's own SSH key for a destination, and "Remove access" (ADR 0012, docs/07). Says
// exactly what changes on the server before doing anything, then shows each step ✓ / ! / ✗.
import { computed, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { AccessReport, Profile_Serialize } from "@/bindings";
import { api, errorMessage } from "@/lib/api";
import { bcp47 } from "@/i18n";
import { sshAccounts, useProfiles } from "@/stores/profiles";
import AppIcon from "./AppIcon.vue";
import ModalDialog from "./ModalDialog.vue";
import StatusIcon from "./StatusIcon.vue";

const props = defineProps<{
  /** The destination; `null` = closed. */
  profile: Profile_Serialize | null;
  action: "install" | "remove";
}>();
const emit = defineEmits<{ close: [] }>();
const { t, te, locale } = useI18n();
const profiles = useProfiles();

const running = ref(false);
const error = ref<string | null>(null);
const report = shallowRef<AccessReport | null>(null);

watch(
  () => props.profile,
  () => {
    report.value = null;
    error.value = null;
  },
);

/** Every account whose authorized_keys changes, e.g. `alice@nas.example.lan and root@pve.example.lan`. */
const who = computed(() => {
  const accounts = props.profile ? sshAccounts(props.profile) : [];
  return new Intl.ListFormat(bcp47(locale.value), { type: "conjunction" }).format(accounts);
});
/** After Remove access the destination signs in with its password, or its key file / agent. */
const byPassword = computed(() => {
  const b = props.profile?.destination.backend;
  return b?.kind === "sftp" && !!b.password;
});
const consequences = computed(() =>
  [1, 2, 3, 4].map((n) => {
    const key = n === 3 && props.action === "remove" && byPassword.value ? "what3Password" : `what${n}`;
    return t(`access.${props.action}.${key}`, { who: who.value });
  }),
);
const done = computed(() => !!report.value);
/** "ok" only when every step is ✓: a "!" (e.g. the server couldn't be reached for the last
 *  check) leaves it done but unconfirmed. */
const outcome = computed<"ok" | "unconfirmed" | "failed" | "failedUndo">(() => {
  const r = report.value;
  if (!r) return "failed";
  if (r.steps.some((s) => s.id === "undone" && s.status === "fail")) return "failedUndo";
  if (!r.profile || r.steps.some((s) => s.status === "fail")) return "failed";
  return r.steps.every((s) => s.status === "ok") ? "ok" : "unconfirmed";
});

function stepLabel(id: string) {
  const key = `access.steps.${id}`;
  return te(key) ? t(key) : id;
}

async function run() {
  if (!props.profile) return;
  running.value = true;
  error.value = null;
  try {
    report.value =
      props.action === "install" ? await api.installAppKey(props.profile.id) : await api.removeAccess(props.profile.id);
    await profiles.load();
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    running.value = false;
  }
}
</script>

<template>
  <ModalDialog
    :open="!!profile"
    :title="$t(`access.${action}.title`, { name: profile?.name ?? '' })"
    role="alertdialog"
    :width="520"
    :busy="running"
    @close="emit('close')"
  >
    <template #title>
      <span class="title"><AppIcon :name="action === 'install' ? 'key' : 'lock'" /> {{ $t(`access.${action}.title`, { name: profile?.name ?? "" }) }}</span>
    </template>

        <template v-if="!done">
          <ul>
            <li v-for="c in consequences" :key="c">{{ c }}</li>
          </ul>
          <p v-if="error" class="error small" role="alert">{{ error }}</p>
        </template>

        <div v-else aria-live="polite">
          <ol class="steps">
            <li v-for="(s, i) in report!.steps" :key="i">
              <StatusIcon :status="s.status" />
              <span><strong>{{ stepLabel(s.id) }}</strong> <span class="muted">{{ s.message }}</span></span>
            </li>
          </ol>
          <p class="small" :class="outcome === 'ok' ? 'ok-text' : 'error'">
            {{ outcome === "failedUndo" ? $t("access.install.failedUndo") : $t(`access.${action}.${outcome}`) }}
          </p>
        </div>

    <template #actions>
          <template v-if="!done">
            <button data-autofocus class="btn" :disabled="running" @click="emit('close')">{{ $t("common.cancel") }}</button>
            <button class="btn" :class="action === 'remove' ? 'btn-danger' : 'btn-primary'" :disabled="running" @click="run">
              {{ running ? $t("access.working") : $t(`access.${action}.confirm`) }}
            </button>
          </template>
          <button v-else data-autofocus class="btn btn-primary" @click="emit('close')">{{ $t("common.close") }}</button>
    </template>
  </ModalDialog>
</template>

<style scoped>
.title { display: inline-flex; gap: var(--sp-2); align-items: center; }
ul { padding-left: 1.2rem; margin: 0; color: var(--text-2); }
li { margin: 0.35rem 0; }
.steps { list-style: none; padding: 0; margin: 0.5rem 0; }
.steps li { display: flex; gap: 0.5rem; align-items: flex-start; }
.steps li > span { min-width: 0; overflow-wrap: anywhere; }
</style>
