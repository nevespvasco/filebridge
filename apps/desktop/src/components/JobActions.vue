<script setup lang="ts">
// Everything a user can do with one transfer, as a ⋮ menu, with the dialogs some of it needs
// (docs/design/specs/D06, "Row menu"). Which action shows comes from the engine's own table
// (`JOB_OPERATIONS`), never a copy.
import { computed, ref } from "vue";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import { JOB_OPERATIONS, type JobState, type JobSummary_Serialize } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { errorMessage } from "@/lib/api";
import { continueJob, jobLabel } from "@/lib/jobs";
import { copyText } from "@/lib/platform";
import { useJobs } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";
import ConfirmDialog from "./ConfirmDialog.vue";
import RowMenu, { type MenuItem } from "./RowMenu.vue";

const props = defineProps<{ job: JobSummary_Serialize; showOpen?: boolean }>();
const emit = defineEmits<{ exportReport: [] }>();
const jobs = useJobs();
const profiles = useProfiles();
const router = useRouter();
const { t } = useI18n();
const fmt = useFmt();

const can = (op: keyof typeof JOB_OPERATIONS) => (JOB_OPERATIONS[op] as readonly JobState[]).includes(props.job.state);
const unsent = computed(() => ["draft", "awaiting_approval", "ready"].includes(props.job.state));

const items = computed<MenuItem[]>(() => {
  const out: MenuItem[] = [];
  if (props.showOpen) out.push({ id: "open", label: t("jobs.menu.open"), icon: "next" });
  if (unsent.value) out.push({ id: "continue", label: t(props.job.kind === "repair" ? "repair.continue" : "jobDetail.continue"), icon: "pointing" });
  if (can("pause")) out.push({ id: "pause", label: t("jobDetail.pause"), icon: "pause" });
  if (props.job.state === "paused") out.push({ id: "resume", label: t("jobDetail.resume"), icon: "play" });
  out.push({ id: "rename", label: t("jobs.menu.rename"), icon: "renamed" });
  const again = can("send_again");
  out.push({ id: "again", label: t("jobs.menu.again"), icon: "again", disabled: !again, reason: t("jobs.menu.whenFinished") });
  // A repair works on a folder of its own destination: it only runs again there.
  if (props.job.kind !== "repair") out.push({ id: "againTo", label: t("jobs.menu.againTo"), icon: "destinations", disabled: !again, reason: t("jobs.menu.whenFinished") });
  out.push({ id: "export", label: t("jobs.menu.export"), icon: "export", disabled: !can("export_report"), reason: t("jobs.menu.whenFinished") });
  out.push({ id: "compare", label: t("report.compare.menu"), icon: "different", disabled: !can("export_report"), reason: t("jobs.menu.whenFinished") });
  out.push({ id: "copyId", label: t("jobs.menu.copyId"), icon: "copy" });
  if (can("cancel")) out.push({ id: "cancel", label: t("jobDetail.cancel"), icon: "close", danger: true });
  return out;
});

const error = ref<string | null>(null);
const busy = ref(false);
const renaming = ref(false);
const name = ref("");
const againOpen = ref(false);
const againTo = ref(false);
const target = ref<string>("");
const cancelling = ref(false);

async function act(fn: () => Promise<void>) {
  busy.value = true;
  error.value = null;
  try {
    await fn();
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}

async function onSelect(id: string) {
  const j = props.job;
  switch (id) {
    case "open":
      return void router.push({ name: "job", params: { id: j.id } });
    case "continue":
      return continueJob(j, router);
    case "pause":
      return act(() => jobs.pause(j.id));
    case "resume":
      return act(() => jobs.resume(j.id));
    case "rename":
      name.value = j.label ?? "";
      renaming.value = true;
      return;
    case "again":
    case "againTo":
      againTo.value = id === "againTo";
      target.value = j.profile_id;
      void profiles.load();
      againOpen.value = true;
      return;
    case "export":
      return emit("exportReport");
    case "compare":
      return void router.push({ name: "compare", params: { a: j.id } });
    case "copyId":
      return act(() => copyText(j.id));
    case "cancel":
      cancelling.value = true;
      return;
  }
}

async function saveName() {
  await act(() => jobs.setLabel(props.job.id, name.value.trim() || null));
  if (!error.value) renaming.value = false;
}

async function sendAgain() {
  await act(async () => {
    const draft = await jobs.sendAgain(props.job.id, againTo.value ? target.value : null);
    if (!draft) return;
    againOpen.value = false;
    await continueJob(draft, router);
  });
}

async function doCancel() {
  await act(() => jobs.cancel(props.job.id));
  cancelling.value = false;
}

const againLines = computed(() => [t("again.explain"), t("again.decisions"), t("again.noDelete")]);
const cancelLines = computed(() => [t("jobDetail.cancelStops"), t("jobDetail.cancelKeeps", { n: fmt.count(props.job.counts.done) }), t("jobDetail.cancelNoDelete"), t("jobDetail.cancelFinal")]);
</script>

<template>
  <span class="job-actions">
    <RowMenu :items="items" :label="$t('jobs.menu.label', { name: jobLabel(job) })" @select="onSelect" />
    <span v-if="error" class="error small" role="alert">{{ error }}</span>

    <ConfirmDialog
      :open="renaming"
      :title="$t('jobs.rename.title')"
      :consequences="[]"
      :confirm-label="$t('common.save')"
      :busy="busy"
      @confirm="saveName"
      @cancel="renaming = false"
    >
      <label class="field">
        <span>{{ $t("review.label") }}</span>
        <input v-model="name" maxlength="120" :placeholder="jobLabel({ label: null, sources: job.sources })" @keydown.enter="saveName" />
        <span class="hint">{{ $t("jobs.rename.hint") }}</span>
      </label>
    </ConfirmDialog>

    <ConfirmDialog
      :open="againOpen"
      :title="$t('again.title', { name: jobLabel(job) })"
      :consequences="againLines"
      :confirm-label="$t('again.continue')"
      :busy="busy"
      @confirm="sendAgain"
      @cancel="againOpen = false"
    >
      <label v-if="againTo" class="field">
        <span>{{ $t("again.destination") }}</span>
        <select v-model="target">
          <option v-for="p in profiles.list" :key="p.id" :value="p.id">{{ p.name }}</option>
        </select>
        <span class="hint">{{ $t("again.otherRules") }}</span>
      </label>
    </ConfirmDialog>

    <ConfirmDialog
      :open="cancelling"
      :title="$t('jobDetail.cancelTitle')"
      :consequences="cancelLines"
      :confirm-label="$t('jobDetail.cancelConfirm')"
      :cancel-label="$t('jobDetail.keepSending')"
      danger
      :busy="busy"
      @confirm="doCancel"
      @cancel="cancelling = false"
    />
  </span>
</template>

<style scoped>
.job-actions { display: inline-flex; gap: 0.5rem; align-items: center; }
</style>
