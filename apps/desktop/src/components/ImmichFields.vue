<script setup lang="ts">
// Immich's part of the destination form (ADR 0020): its address, an administrator's API key
// (it goes to the system keyring, never into the profile), and where the destination folder is
// in Immich's external library. The checks' results show ✓ / ! / ✗; when more than one library
// could take the folder, the user picks one.
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { DetectNextcloudResponse_Serialize } from "@/bindings";
import AppIcon from "./AppIcon.vue";
import HelpTip from "./HelpTip.vue";
import StatusIcon from "./StatusIcon.vue";

export type ImmichForm = { server: string; importPath: string; libraryId: string };

const fields = defineModel<ImmichForm>("fields", { required: true });
/** Kept apart from the other fields and cleared by the form once used. */
const apiKey = defineModel<string>("apiKey", { required: true });
const props = defineProps<{ result: DetectNextcloudResponse_Serialize | null; folder: string }>();
const { t, te } = useI18n();

/** http:// is refused by the service off this machine; say so before asking it. */
const plainHttp = computed(() => /^http:\/\//i.test(fields.value.server.trim()) && !/^http:\/\/(localhost|127\.|\[::1\])/i.test(fields.value.server.trim()));

function stepLabel(id: string) {
  const key = `immich.steps.${id}`;
  return te(key) ? t(key) : id;
}

/** The libraries the service listed, as `<id> <name> (<import paths>)`. */
const libraries = computed(() =>
  (props.result?.profile ? [] : props.result?.containers ?? []).map((line) => {
    const i = line.indexOf(" ");
    return { id: line.slice(0, i), label: line.slice(i + 1) };
  }),
);
</script>

<template>
  <div class="immich">
    <label class="field">
      <span class="label-line">{{ $t("immich.server") }}<span class="req" aria-hidden="true">*</span>
        <HelpTip :text="$t('immich.help.server')" example="photos.example.com" :label="$t('immich.server')" />
      </span>
      <input v-model="fields.server" spellcheck="false" autocomplete="url" placeholder="photos.example.com" />
      <span v-if="plainHttp" class="hint warn-text">{{ $t("immich.https") }}</span>
    </label>
    <label class="field">
      <span class="label-line">{{ $t("immich.apiKey") }}<span class="req" aria-hidden="true">*</span>
        <HelpTip :text="$t('immich.help.apiKey')" :label="$t('immich.apiKey')" />
      </span>
      <input v-model="apiKey" type="password" autocomplete="new-password" spellcheck="false" />
      <span class="hint"><AppIcon name="lock" :size="16" /> {{ $t("immich.hint.apiKey") }}</span>
    </label>
    <label class="field">
      <span class="label-line">{{ $t("immich.importPath") }}
        <HelpTip :text="$t('immich.help.importPath')" example="/mnt/photos" :label="$t('immich.importPath')" />
      </span>
      <input v-model="fields.importPath" spellcheck="false" :placeholder="folder.trim() || '/mnt/photos'" />
      <span class="hint">{{ $t("immich.hint.importPath") }}</span>
    </label>
    <label class="field">
      <span class="label-line">{{ $t("immich.library") }}
        <HelpTip :text="$t('immich.help.library')" :label="$t('immich.library')" />
      </span>
      <input v-model="fields.libraryId" spellcheck="false" :placeholder="$t('immich.libraryHint')" />
    </label>

    <div v-if="result" class="result wide" aria-live="polite">
      <ol class="steps">
        <li v-for="s in result.steps" :key="s.id">
          <StatusIcon :status="s.status" />
          <span><strong>{{ stepLabel(s.id) }}</strong> <span class="muted">{{ s.message }}</span></span>
        </li>
      </ol>
      <div v-if="libraries.length" class="libraries">
        <p class="small">{{ $t("immich.pickLibrary") }}</p>
        <button v-for="l in libraries" :key="l.id" type="button" class="btn btn-small" :aria-pressed="fields.libraryId === l.id" @click="fields.libraryId = l.id">
          {{ l.label }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* Its fields sit in the form's grid. */
.immich { display: contents; }
.wide { grid-column: 1 / -1; }
.steps { list-style: none; padding: 0; margin: 0.5rem 0 0; }
.steps li { display: flex; gap: 0.5rem; align-items: flex-start; margin: 0.45rem 0; }
.libraries { display: flex; flex-wrap: wrap; gap: 0.5rem; align-items: center; }
.libraries p { flex-basis: 100%; margin: 0; }
.btn[aria-pressed="true"] { border-color: var(--accent); background: var(--accent-soft); }
</style>
