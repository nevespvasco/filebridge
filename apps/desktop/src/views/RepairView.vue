<script setup lang="ts">
// Repair destination (docs/04, "Repair destination"): names already at a destination go through
// the same rules and decisions as a send, then are renamed there. Reads the folder, shows the
// names by rule, and renames only after a confirmation that says exactly what happens.
import { computed, onMounted, provide, reactive, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import type { DecisionInput, RuleCount } from "@/bindings";
import AppIcon from "@/components/AppIcon.vue";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import IssueGroup from "@/components/IssueGroup.vue";
import { useFmt } from "@/composables/useFmt";
import { api, errorMessage } from "@/lib/api";
import { DECISIONS, type Decisions } from "@/lib/decisions";
import { useJobs } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";

const props = defineProps<{ id: string }>();
const jobs = useJobs();
const profiles = useProfiles();
const router = useRouter();
const { t } = useI18n();
const fmt = useFmt();

const job = computed(() => jobs.byId[props.id]);
const profile = computed(() => (job.value ? profiles.byId[job.value.profile_id] : undefined));
const folder = computed(() => job.value?.options.target_dir ?? null);
const where = computed(() => (folder.value ? `/${folder.value}` : t("repair.root")));
const scan = computed(() => jobs.live[props.id]?.scan);

const rules = ref<RuleCount[]>([]);
const analysing = ref(false);
const error = ref<string | null>(null);
const confirming = ref(false);
const starting = ref(false);

const pending = computed(() => rules.value.reduce((n, r) => n + r.pending, 0));
/** Entries that will really change with the decisions as they are (not issues: a name can have
 *  several, and a kept or skipped one changes nothing). */
const changes = ref(0);
const deciding = computed(() => job.value?.state === "awaiting_approval" || job.value?.state === "ready");

async function refreshRules() {
  rules.value = await api.ruleCounts(props.id, null);
  void jobs.refresh(props.id);
}

async function analyse() {
  analysing.value = true;
  error.value = null;
  try {
    rules.value = (await api.analyse(props.id)).rules;
    await jobs.refresh(props.id);
  } catch (e) {
    error.value = errorMessage(e);
    await jobs.refresh(props.id);
  } finally {
    analysing.value = false;
  }
}

// The issue lists decide on this job, not on the New transfer wizard's.
const decisions: Decisions = reactive({
  selectionGroup: null as string | null,
  async decide(issueId: number, decision: DecisionInput) {
    await api.decide(props.id, issueId, decision);
    await refreshRules();
  },
  async decideAll(rule: string, decision: DecisionInput, query: string | null = null) {
    try {
      return await api.decideAll(props.id, rule, decision, query);
    } finally {
      await refreshRules();
    }
  },
  async decideMany(ids: number[], decision: DecisionInput) {
    try {
      return await api.decideMany(props.id, ids, decision);
    } finally {
      await refreshRules();
    }
  },
});
provide(DECISIONS, decisions);

onMounted(async () => {
  await Promise.all([jobs.refresh(props.id), profiles.load()]);
  const state = job.value?.state;
  if (state === "draft") await analyse();
  else if (state === "awaiting_approval" || state === "ready") await refreshRules();
  else if (state) void router.replace({ name: "job", params: { id: props.id } });
});

const consequences = computed(() => [
  t("repair.confirm.where", { destination: profile.value?.name ?? "—", folder: where.value }),
  t("repair.confirm.onlyNames"),
  t("repair.confirm.taken"),
  ...(profile.value?.integration === "nextcloud" ? [t("repair.confirm.nextcloud")] : []),
  ...(profile.value?.integration === "immich" ? [t("repair.confirm.immich")] : []),
]);

async function openConfirm() {
  error.value = null;
  try {
    changes.value = await api.plannedRenames(props.id);
    confirming.value = true;
  } catch (e) {
    error.value = errorMessage(e);
  }
}

async function rename() {
  starting.value = true;
  error.value = null;
  try {
    await api.start(props.id);
    await jobs.refresh(props.id);
    confirming.value = false;
    void router.push({ name: "job", params: { id: props.id } });
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    starting.value = false;
  }
}
</script>

<template>
  <div class="repair">
    <header class="page-head">
      <div>
        <h1>{{ $t("repair.title") }}</h1>
        <p class="lead-text">{{ $t("repair.lead", { destination: profile?.name ?? "—", folder: where }) }}</p>
      </div>
    </header>

    <p v-if="error" class="error" role="alert">
      {{ error }}
      <button v-if="job?.state === 'draft'" class="btn btn-small" @click="analyse">{{ $t("common.retry") }}</button>
    </p>

    <section v-if="analysing" class="card reading" aria-live="polite">
      <AppIcon name="search" :size="24" class="muted" />
      <div>
        <strong>{{ $t("repair.reading") }}</strong>
        <p class="small muted num">{{ $t("repair.readCount", { n: fmt.count(scan?.files ?? 0) }, scan?.files ?? 0) }}</p>
        <p v-if="scan?.current" class="small muted mono ellipsis">{{ scan.current }}</p>
      </div>
    </section>

    <template v-else-if="deciding">
      <section v-if="!rules.length" class="card clean" role="status">
        <AppIcon name="checkCircle" :size="28" class="ok-text" />
        <div>
          <strong>{{ $t("repair.clean") }}</strong>
          <p class="small muted">{{ $t("repair.cleanBody", { n: fmt.count(job?.counts.files ?? 0) }, job?.counts.files ?? 0) }}</p>
        </div>
      </section>
      <template v-else>
        <p class="small muted">{{ $t("repair.howTo") }}</p>
        <div class="groups">
          <IssueGroup v-for="r in rules" :key="`${id}-${r.rule}`" :job-id="id" :rule="r" />
        </div>
      </template>
    </template>

    <div class="footer">
      <RouterLink class="btn" :to="{ name: 'destinations' }"><AppIcon name="back" /> {{ $t("nav.destinations") }}</RouterLink>
      <span class="grow" />
      <span v-if="pending" class="why-disabled">{{ $t("repair.pending", { n: fmt.count(pending) }, pending) }}</span>
      <button class="btn btn-primary" :disabled="analysing || !deciding || pending > 0 || !rules.length" @click="openConfirm">
        <AppIcon name="renamed" /> {{ $t("repair.rename") }}
      </button>
    </div>

    <ConfirmDialog
      :open="confirming"
      :title="$t('repair.confirm.title', { n: fmt.count(changes) }, changes)"
      :consequences="consequences"
      :confirm-label="$t('repair.rename')"
      :busy="starting"
      @confirm="rename"
      @cancel="confirming = false"
    />
  </div>
</template>

<style scoped>
.repair { display: grid; gap: 1rem; }
.reading, .clean { display: flex; gap: 1rem; align-items: center; padding: 1.25rem; }
.reading p, .clean p { margin: 0.2rem 0 0; }
.groups { display: grid; gap: 0.75rem; }
.grow { flex: 1; }
</style>
