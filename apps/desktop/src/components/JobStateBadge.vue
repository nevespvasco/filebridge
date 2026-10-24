<script setup lang="ts">
// A transfer's state as icon + word + colour, never colour alone (docs/design/01-design-system.md,
// "Status"). Given the whole job, a finished one says what matters most: "Verified" when every
// file was compared with no difference, or how many files need a look.
import { computed } from "vue";
import type { JobState, JobSummary_Serialize } from "@/bindings";
import type { IconName } from "@/lib/icons";
import AppIcon from "./AppIcon.vue";

const props = defineProps<{ state: JobState; job?: Pick<JobSummary_Serialize, "state" | "options" | "counts"> }>();

const ICON: Record<JobState, IconName> = {
  draft: "pending",
  analysing: "search",
  awaiting_approval: "hand",
  ready: "pending",
  transferring: "sending",
  paused: "pause",
  retrying: "retry",
  waiting: "waiting",
  verifying: "checksum",
  integrating: "sending",
  completed: "checkCircle",
  completed_with_warnings: "warning",
  cancelled: "close",
  failed: "failCircle",
};

const view = computed<{ tone: string; icon: IconName; key?: string; n?: number }>(() => {
  const j = props.job;
  const problems = j ? j.counts.failed + j.counts.mismatch : 0;
  if (j && (props.state === "completed" || props.state === "completed_with_warnings")) {
    if (problems > 0) return { tone: "warn", icon: "warning", key: "state.problems", n: problems };
    if (j.options.verify && props.state === "completed") return { tone: "ok", icon: "shield", key: "state.verified" };
  }
  switch (props.state) {
    case "completed":
      return { tone: "ok", icon: ICON[props.state] };
    case "completed_with_warnings":
    case "awaiting_approval":
    case "retrying":
    case "waiting":
    case "paused":
      return { tone: "warn", icon: ICON[props.state] };
    case "failed":
      return { tone: "danger", icon: ICON[props.state] };
    case "transferring":
    case "verifying":
    case "integrating":
    case "analysing":
      return { tone: "accent", icon: ICON[props.state] };
    default:
      return { tone: "muted", icon: ICON[props.state] };
  }
});
</script>

<template>
  <span class="badge state-badge" :class="`badge-${view.tone}`">
    <AppIcon :name="view.icon" :size="13" weight="bold" />
    {{ view.key ? $t(view.key, { n: view.n }, view.n ?? 1) : $t(`state.${state}`) }}
  </span>
</template>

<style scoped>
.state-badge { display: inline-flex; align-items: center; gap: 0.3rem; }
</style>
