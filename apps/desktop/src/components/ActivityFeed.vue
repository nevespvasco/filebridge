<script setup lang="ts">
// Milestones of every transfer and destination changes, newest first (docs/design/specs/D07,
// "Activity"). Read from the service's log, so it covers what happened with the window closed.
import { onMounted, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { ActivityItem } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { api } from "@/lib/api";
import { jobLabel, repairLabel } from "@/lib/jobs";
import type { IconName } from "@/lib/icons";
import { useConnection } from "@/stores/connection";
import { useJobs } from "@/stores/jobs";
import AppIcon from "./AppIcon.vue";
import LoadError from "./LoadError.vue";
import RelativeTime from "./RelativeTime.vue";

const props = withDefaults(defineProps<{ limit?: number }>(), { limit: 8 });
const jobs = useJobs();
const conn = useConnection();
const { t } = useI18n();
const fmt = useFmt();

const items = shallowRef<ActivityItem[]>([]);
const loaded = shallowRef(false);
const failed = shallowRef(false);

async function load() {
  try {
    items.value = await api.activity(null, props.limit);
    failed.value = false;
  } catch {
    failed.value = true;
  } finally {
    loaded.value = true;
  }
}
onMounted(load);
// Back after the engine was down: read again what it couldn't answer meanwhile.
watch(
  () => conn.connected,
  (up) => {
    if (up && failed.value) void load();
  },
);
// A job changing state is when a milestone may have been written.
let timer: ReturnType<typeof setTimeout> | undefined;
watch(
  () => Object.values(jobs.byId).map((j) => `${j.id}:${j.state}`).join(","),
  () => {
    clearTimeout(timer);
    timer = setTimeout(load, 800);
  },
);

type Line = { icon: IconName; tone: string; text: string; detail?: string };
function describe(a: ActivityItem): Line {
  const d = (a.data ?? {}) as Record<string, unknown>;
  // A Repair job has no sources to name it by: "Repair · <folder>", as everywhere else.
  const known = a.job_id ? jobs.byId[a.job_id] : undefined;
  const name = a.job_label ?? (known ? jobLabel(known) : typeof d.repair === "string" ? repairLabel(d.repair || null) : "—");
  const destination = a.profile_name ?? "—";
  switch (a.kind) {
    case "created":
      if (typeof d.repair === "string") {
        return { icon: "renamed", tone: "muted", text: t("activity.createdRepair", { folder: d.repair ? `/${d.repair}` : t("repair.root"), destination }) };
      }
      return { icon: "plus", tone: "muted", text: t("activity.created", { name }) };
    case "state":
      return { icon: "send", tone: "accent", text: t("activity.started", { name, destination }) };
    case "retry":
      return { icon: "retry", tone: "warn", text: t("activity.retrying", { destination }), detail: name };
    case "send_window_wait":
      return { icon: "waiting", tone: "muted", text: t("activity.waiting", { name }) };
    case "finished": {
      const state = String(d.state ?? "");
      if (state === "completed") return { icon: "checkCircle", tone: "ok", text: t("activity.sent", { destination }), detail: name };
      if (state === "completed_with_warnings") return { icon: "warning", tone: "warn", text: t("activity.sentWarnings", { destination, n: Number(d.warnings ?? 0) }), detail: name };
      if (state === "cancelled") return { icon: "close", tone: "muted", text: t("activity.cancelled", { name }) };
      return { icon: "failCircle", tone: "danger", text: t("activity.failed", { name }), detail: typeof d.error === "string" ? d.error : undefined };
    }
    case "profile":
      return d.action === "removed"
        ? { icon: "destinations", tone: "muted", text: t("activity.profileRemoved", { name: destination }) }
        : { icon: "destinations", tone: "muted", text: t("activity.profileAdded", { name: destination }) };
    default:
      return { icon: "info", tone: "muted", text: t("joblog.unknown", { kind: a.kind }) };
  }
}
void fmt;
</script>

<template>
  <div class="activity">
    <LoadError v-if="failed && !(conn.offline && items.length)" compact :message="$t('activity.error')" @retry="load" />
    <p v-else-if="loaded && !items.length" class="small muted">{{ $t("activity.empty") }}</p>
    <ol v-else class="list">
      <li v-for="a in items" :key="a.id">
        <AppIcon :name="describe(a).icon" :class="`tone-${describe(a).tone}`" />
        <div class="text">
          <component :is="a.job_id ? 'RouterLink' : 'span'" :to="a.job_id ? { name: 'job', params: { id: a.job_id } } : undefined" class="what">
            {{ describe(a).text }}
          </component>
          <span class="small muted">
            <template v-if="describe(a).detail">{{ describe(a).detail }} · </template><RelativeTime :at="a.at" />
          </span>
        </div>
      </li>
    </ol>
  </div>
</template>

<style scoped>
.list { list-style: none; margin: 0; padding: 0; display: grid; }
.list li { display: grid; grid-template-columns: 1.2rem minmax(0, 1fr); gap: 0.55rem; padding: 0.45rem 0; border-bottom: 1px solid var(--rule); }
.list li:last-child { border-bottom: 0; }
.text { display: grid; min-width: 0; }
.what { color: var(--text); text-decoration: none; overflow-wrap: anywhere; }
a.what:hover { text-decoration: underline; }
.tone-ok { color: var(--ok); }
.tone-warn { color: var(--warn-strong); }
.tone-danger { color: var(--danger); }
.tone-accent { color: var(--accent); }
.tone-muted { color: var(--text-2); }
</style>
