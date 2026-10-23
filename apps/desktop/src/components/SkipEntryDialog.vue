<script setup lang="ts">
// "Skip this file" during a send (docs/design/specs/D04): says what it does, then asks the engine
// (`skip_entry`). The file stays Skipped for this transfer; only a new one brings it back.
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { api, errorMessage } from "@/lib/api";
import { useJobs } from "@/stores/jobs";
import ConfirmDialog from "./ConfirmDialog.vue";

const props = defineProps<{ jobId: string; entry: { idx: number; name: string } | null }>();
const emit = defineEmits<{ done: []; cancel: [] }>();
const { t } = useI18n();
const jobs = useJobs();
const busy = ref(false);
const error = ref<string | null>(null);
watch(
  () => props.entry,
  () => (error.value = null),
);

const consequences = computed(() => {
  const name = props.entry?.name ?? "";
  const sending = jobs.byId[props.jobId]?.state === "transferring";
  return [t("skip.notSent", { name }), ...(sending ? [t("skip.restart")] : []), t("skip.stays")];
});

async function confirm() {
  if (!props.entry) return;
  busy.value = true;
  try {
    await api.skipEntry(props.jobId, props.entry.idx);
    void jobs.refresh(props.jobId);
    emit("done");
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <ConfirmDialog
    :open="entry != null"
    :title="$t('skip.title', { name: entry?.name ?? '' })"
    :consequences="consequences"
    :confirm-label="$t('skip.confirm')"
    :busy="busy"
    @confirm="confirm"
    @cancel="emit('cancel')"
  >
    <p v-if="error" class="error small" role="alert">{{ error }}</p>
  </ConfirmDialog>
</template>
