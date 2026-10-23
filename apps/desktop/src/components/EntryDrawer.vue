<script setup lang="ts">
// Everything about one file of a transfer: its names, where it is at both ends, and the checksum
// that proves it arrived intact (docs/design/specs/D05, "Entry detail").
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { JOB_OPERATIONS, type EntryDetail_Serialize, type JobState } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { api, errorMessage } from "@/lib/api";
import { CATEGORY_ICON, categoryColor } from "@/lib/categories";
import { canReveal, copyText, revealInFileManager } from "@/lib/platform";
import { knownThumbnail, loadThumbnail } from "@/lib/thumbnails";
import { useJobs } from "@/stores/jobs";
import AppIcon from "./AppIcon.vue";
import BytesDiff from "./BytesDiff.vue";
import Drawer from "./Drawer.vue";
import EntryStatusBadge from "./EntryStatusBadge.vue";
import SkipEntryDialog from "./SkipEntryDialog.vue";

const props = defineProps<{ jobId: string; idx: number | null }>();
const jobs = useJobs();
/** A Repair job's entries were found at the destination: there is no local file to show. */
const remote = computed(() => jobs.byId[props.jobId]?.kind === "repair");
const emit = defineEmits<{ close: [] }>();
const { t, te } = useI18n();
const fmt = useFmt();

const detail = ref<EntryDetail_Serialize | null>(null);
/** A preview of the source image, when it has one (ADR 0027). */
const preview = ref<string | null>(null);
const error = ref<string | null>(null);
const copied = ref<string | null>(null);

async function load(job: string, idx: number | null) {
  detail.value = null;
  preview.value = null;
  error.value = null;
  if (idx == null) return;
  try {
    detail.value = await api.entry(job, idx);
  } catch (e) {
    error.value = errorMessage(e);
    return;
  }
  if (remote.value || detail.value.view.is_dir || detail.value.view.category !== "image") return;
  const had = knownThumbnail(job, idx);
  const url = had !== undefined ? had : await loadThumbnail(job, idx).promise;
  if (props.jobId === job && props.idx === idx) preview.value = url;
}
watch(() => [props.jobId, props.idx] as const, ([job, idx]) => load(job, idx), { immediate: true });

// A file not sent yet of a started send can be left out (docs/design/specs/D04).
const skipping = ref(false);
const canSkip = computed(() => {
  const state = jobs.byId[props.jobId]?.state;
  const status = detail.value?.view.status;
  return !remote.value && !detail.value?.view.is_dir && (status === "not_sent" || status === "sending") && !!state && (JOB_OPERATIONS.skip_entry as readonly JobState[]).includes(state);
});
function skipped() {
  skipping.value = false;
  void load(props.jobId, props.idx);
}

const v = computed(() => detail.value?.view);
const extension = computed(() => {
  const n = v.value?.name ?? "";
  const dot = n.lastIndexOf(".");
  return dot > 0 ? n.slice(dot) : "";
});
/** The hash in groups of 4 for reading; copied whole. */
const grouped = computed(() => detail.value?.hash?.hex.match(/.{1,4}/g)?.join(" ") ?? "");
const verifyText = computed(() => {
  switch (v.value?.status) {
    case "verified":
      return t("entry.verify.same");
    case "different":
      return t("entry.verify.different");
    case "missing":
      return t("entry.verify.missing");
    case "unverifiable":
      return t("entry.verify.unverifiable");
    default:
      return t("entry.verify.notYet");
  }
});
const hashNone = computed(() => (v.value?.status === "sent" ? t("entry.hashNoVerify") : t("entry.hashNotYet")));

function ruleLabel(rule: string): string {
  const key = `ruleName.${rule}`;
  return te(key) ? t(key) : rule;
}

async function copy(what: string, text: string) {
  try {
    await copyText(text);
    copied.value = what;
    setTimeout(() => (copied.value = null), 2500);
  } catch (e) {
    error.value = errorMessage(e);
  }
}
async function reveal() {
  if (!detail.value) return;
  try {
    await revealInFileManager(detail.value.source_path);
  } catch (e) {
    error.value = errorMessage(e);
  }
}
</script>

<template>
  <Drawer :open="idx != null" :title="v?.name ?? $t('entry.title')" @close="emit('close')">
    <p v-if="error" class="error small" role="alert">{{ error }}</p>
    <p v-else-if="!detail" class="small muted">{{ $t("common.loading") }}</p>
    <template v-else-if="v">
      <img v-if="preview" class="preview" :src="preview" :alt="$t('entry.preview', { name: v.name })" />
      <div class="hero">
        <AppIcon :name="v.is_dir ? 'folder' : CATEGORY_ICON[v.category]" :size="40" :style="{ color: v.is_dir ? 'var(--text-2)' : categoryColor(v.category) }" />
        <div>
          <strong class="name">{{ v.name }}</strong>
          <div class="small muted">
            {{ $t(`category.${v.category}`) }} · <span class="num">{{ fmt.bytes(v.size) }}</span> ·
            <EntryStatusBadge :status="v.status" />
          </div>
        </div>
      </div>

      <section>
        <h3>{{ $t("entry.name") }}</h3>
        <dl class="kv">
          <dt>{{ $t("entry.atSource") }}</dt>
          <dd>{{ v.source_name ?? v.name }}</dd>
          <dt>{{ $t("entry.atDestination") }}</dt>
          <dd>{{ v.name }}</dd>
          <template v-if="v.kept_as">
            <dt>{{ $t("entry.olderVersion") }}</dt>
            <dd>{{ $t("entry.keptAs", { name: v.kept_as }) }}</dd>
          </template>
          <template v-if="detail.issues.length">
            <dt>{{ $t("entry.changedBy") }}</dt>
            <dd>
              <div v-for="i in detail.issues" :key="i.id" class="rule">
                {{ ruleLabel(i.rule) }} — <span class="muted">{{ $t(`decision.${i.decision}`) }}</span>
                <BytesDiff v-if="i.before.length && i.after.length" :before="i.before" :after="i.after" />
              </div>
            </dd>
          </template>
        </dl>
      </section>

      <section>
        <h3>{{ $t("entry.where") }}</h3>
        <dl class="kv">
          <dt>{{ $t("entry.source") }}</dt>
          <dd class="with-copy">
            <span class="path">{{ detail.source_path }}</span>
            <button class="btn btn-small" :aria-label="$t('entry.copySource')" :title="$t('entry.copySource')" @click="copy('source', detail.source_path)"><AppIcon name="copy" /></button>
            <span v-if="detail.source_path_hex" class="badge badge-warn">{{ $t("entry.lossy") }}</span>
            <span v-if="detail.source_path_hex" class="hash">{{ detail.source_path_hex }}</span>
          </dd>
          <dt>{{ $t("entry.destination") }}</dt>
          <dd class="with-copy">
            <template v-if="detail.dest_path">
              <span class="path">{{ detail.dest_path }}</span>
              <button class="btn btn-small" :aria-label="$t('entry.copyDestination')" :title="$t('entry.copyDestination')" @click="copy('dest', detail.dest_path)"><AppIcon name="copy" /></button>
            </template>
            <span v-else class="muted">—</span>
          </dd>
        </dl>
      </section>

      <section>
        <h3>{{ $t("entry.parameters") }}</h3>
        <dl class="kv">
          <dt>{{ $t("entry.size") }}</dt>
          <dd class="num">{{ $t("entry.bytes", { n: fmt.count(v.size) }) }}</dd>
          <dt>{{ $t("entry.modified") }}</dt>
          <dd class="num">{{ detail.mtime ? fmt.date(detail.mtime) : "—" }}</dd>
          <dt>{{ $t("entry.category") }}</dt>
          <dd>{{ $t(`category.${v.category}`) }}<span v-if="extension" class="muted"> ({{ extension }})</span></dd>
          <dt>{{ $t("entry.verification") }}</dt>
          <dd>{{ verifyText }}</dd>
          <template v-if="detail.error">
            <dt>{{ $t("entry.error") }}</dt>
            <dd class="error">{{ detail.error }}</dd>
          </template>
          <dt><AppIcon name="checksum" /> {{ detail.hash?.algo.toUpperCase() ?? "SHA-256" }}</dt>
          <dd class="with-copy">
            <template v-if="detail.hash">
              <span class="hash" :aria-label="detail.hash.hex">{{ grouped }}</span>
              <button class="btn btn-small" :aria-label="$t('entry.copyHash')" :title="$t('entry.copyHash')" @click="copy('hash', detail.hash.hex)"><AppIcon name="copy" /></button>
            </template>
            <span v-else class="muted small">{{ hashNone }}</span>
          </dd>
        </dl>
      </section>

      <p v-if="copied" class="small ok-text" role="status">{{ $t("entry.copied") }}</p>
      <button v-if="!remote && canReveal()" class="btn" @click="reveal"><AppIcon name="reveal" /> {{ $t("entry.reveal") }}</button>
      <button v-if="canSkip" class="btn" @click="skipping = true"><AppIcon name="skipped" /> {{ $t("skip.confirm") }}</button>
      <SkipEntryDialog :job-id="jobId" :entry="skipping && v ? { idx: v.idx, name: v.name } : null" @done="skipped" @cancel="skipping = false" />
    </template>
  </Drawer>
</template>

<style scoped>
h3 { font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.04em; color: var(--text-2); margin: 0 0 0.4rem; }
.preview { display: block; max-width: 100%; max-height: 16rem; margin: 0 auto; border-radius: 6px; background: var(--surface-2); }
.hero { display: flex; gap: 0.9rem; align-items: center; }
.name { font-size: 1.05rem; overflow-wrap: anywhere; }
.kv { display: grid; grid-template-columns: 8.5rem minmax(0, 1fr); gap: 0.45rem 0.8rem; margin: 0; }
dt { color: var(--text-2); font-size: var(--fs-sm); display: inline-flex; gap: 0.3rem; align-items: center; }
dd { margin: 0; min-width: 0; overflow-wrap: anywhere; }
.with-copy { display: flex; gap: 0.4rem; align-items: flex-start; flex-wrap: wrap; }
.with-copy .path, .with-copy .hash { flex: 1; min-width: 0; }
.rule + .rule { margin-top: 0.4rem; }
</style>
