<script setup lang="ts">
// Whether this transfer's sources can be freed at the source, and why not
// (docs/design/specs/D10-reclaimable-source.md). filebridge only shows it and opens the folder:
// it never deletes anything (ADR 0011).
import { computed, onMounted, watch } from "vue";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import type { ReclaimBlocker } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { canReveal, revealInFileManager } from "@/lib/platform";
import { useJobs } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";
import { useReclaim } from "@/stores/reclaim";
import AppIcon from "./AppIcon.vue";
import ProgressBar from "./ProgressBar.vue";
import RelativeTime from "./RelativeTime.vue";

const props = defineProps<{ jobId: string }>();
const reclaim = useReclaim();
const jobs = useJobs();
const profiles = useProfiles();
const router = useRouter();
const { t } = useI18n();
const fmt = useFmt();

const DAY = 24 * 3_600_000;
const job = computed(() => jobs.byId[props.jobId]);
const s = computed(() => reclaim.byJob[props.jobId]);
const checking = computed(() => !!reclaim.checking[props.jobId]);
const progress = computed(() => jobs.live[props.jobId]?.reclaim);

onMounted(async () => {
  const st = await reclaim.status(props.jobId);
  // Re-check by itself when the last check is old and the send is recent (cheap enough then).
  const recent = job.value?.finished_at != null && Date.now() - job.value.finished_at < 30 * DAY;
  const old = !st?.checked_at || Date.now() - st.checked_at > DAY;
  const worth = st && !st.blockers.some((b) => b.code === "not_completed" || b.code === "not_verified");
  if (recent && old && worth) void reclaim.recheck(props.jobId);
});
watch(() => job.value?.state, () => void reclaim.status(props.jobId));

/** What to offer for each blocker. */
function action(code: ReclaimBlocker): { label: string; run: () => void } | null {
  switch (code) {
    case "not_checked":
    case "stale_check":
    case "changed":
    case "source_unavailable":
      return { label: t("reclaim.checkAgain"), run: () => void reclaim.recheck(props.jobId) };
    case "different":
    case "unverifiable":
    case "failed":
      return { label: t("reclaim.seeThem"), run: () => void router.replace({ query: { tab: "files", status: "problems" } }) };
    case "skipped":
      return { label: t("reclaim.seeThem"), run: () => void router.replace({ query: { tab: "files", status: "skipped" } }) };
    default:
      return null;
  }
}
const destination = computed(() => (job.value ? (profiles.byId[job.value.profile_id]?.name ?? "") : ""));
/** Not offered at all when the transfer didn't complete: it would only list why not. */
const shown = computed(() => !!s.value && !s.value.blockers.some((b) => b.code === "not_completed"));
const allBlockers = computed(() => [...(s.value?.blockers ?? []), ...(s.value?.sources.flatMap((src) => src.blockers) ?? [])]);
</script>

<template>
  <section v-if="shown && s" id="reclaim" class="card reclaim" :class="{ ok: s.eligible }" :aria-labelledby="`reclaim-${jobId}`">
    <header class="head">
      <AppIcon :name="s.eligible ? 'broom' : 'info'" :size="20" />
      <h2 :id="`reclaim-${jobId}`">{{ s.eligible ? $t("reclaim.title") : $t("reclaim.notYet") }}</h2>
    </header>

    <template v-if="s.eligible">
      <p>
        {{ $t("reclaim.body", { files: fmt.count(s.sources.reduce((n, x) => n + x.files, 0)), destination, date: fmt.date(s.verified_at) }) }}
        <span class="muted">({{ $t("reclaim.checked") }} <RelativeTime v-if="s.checked_at" :at="s.checked_at" />)</span>
      </p>
      <p v-if="s.size_only" class="small muted">{{ $t("reclaim.sizeOnly") }}</p>
      <ul class="sources">
        <li v-for="src in s.sources" :key="src.path">
          <span class="path">{{ src.path }}</span>
          <span class="small muted num">{{ $t("reclaim.size", { files: fmt.count(src.files), size: fmt.bytes(src.bytes) }) }}</span>
          <button v-if="canReveal()" class="btn btn-small" @click="revealInFileManager(src.path)"><AppIcon name="reveal" /> {{ $t("entry.revealFile") }}</button>
        </li>
      </ul>
      <p class="never small"><AppIcon name="lock" /> {{ $t("reclaim.never") }}</p>
    </template>

    <template v-else>
      <ul class="blockers">
        <li v-for="(b, i) in allBlockers" :key="`${b.code}-${i}`">
          <AppIcon :name="b.code === 'stale_check' ? 'warnCircle' : 'failCircle'" :class="b.code === 'stale_check' ? 'warn-text' : 'error'" />
          <span class="grow">{{ $t(`reclaim.blocker.${b.code}`, { n: fmt.count(b.count ?? 0) }, b.count ?? 0) }}</span>
          <button v-if="action(b.code)" class="btn btn-small" :disabled="checking" @click="action(b.code)!.run()">{{ action(b.code)!.label }}</button>
        </li>
      </ul>
      <details v-if="s.changed_sample.length" class="small">
        <summary>{{ $t("reclaim.changedList") }}</summary>
        <ul class="path">
          <li v-for="p in s.changed_sample" :key="p">{{ p }}</li>
        </ul>
      </details>
    </template>

    <div v-if="checking" class="progress" aria-live="polite">
      <ProgressBar :value="progress?.total ? fmt.percent(progress.checked, progress.total) : 0" :indeterminate="!progress?.total" />
      <span class="small muted num">{{ $t("reclaim.checking", { done: fmt.count(progress?.checked ?? 0), total: fmt.count(progress?.total ?? 0) }) }}</span>
    </div>

    <footer v-if="s.eligible" class="foot small">
      <span v-if="s.dismissed" class="muted">{{ $t("reclaim.hidden") }}</span>
      <button class="link" @click="reclaim.dismiss(jobId, !s.dismissed)">{{ s.dismissed ? $t("reclaim.show") : $t("reclaim.hide") }}</button>
    </footer>
  </section>
</template>

<style scoped>
.reclaim { display: grid; gap: 0.6rem; margin-top: 1rem; }
.reclaim.ok { border-color: color-mix(in srgb, var(--ok) 45%, var(--border)); }
.head { display: flex; gap: 0.5rem; align-items: center; }
.head h2 { font-size: var(--fs-base); margin: 0; }
.ok .head { color: var(--ok); }
p { margin: 0; }
.sources, .blockers { list-style: none; margin: 0; padding: 0; display: grid; gap: 0.4rem; }
.sources li { display: grid; grid-template-columns: minmax(0, 1fr) auto auto; gap: 0.75rem; align-items: center; padding: 0.35rem 0; border-bottom: 1px solid var(--rule); }
.blockers li { display: flex; gap: 0.5rem; align-items: center; }
.grow { flex: 1; }
.never { display: flex; gap: 0.4rem; align-items: center; color: var(--text-2); }
.progress { display: grid; gap: 0.3rem; }
.foot { display: flex; gap: 0.75rem; justify-content: flex-end; }
.link { all: unset; cursor: pointer; color: var(--accent); }
.link:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
</style>
