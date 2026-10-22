<script setup lang="ts">
// One name rule's issues ("312 names with decomposed accents"). Rows are paged from SQLite with
// `after_id`, so a rule with hundreds of thousands of issues never loads at once (docs/06).
import AppIcon from "@/components/AppIcon.vue";
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { DecisionInput, IssueDecision, IssueView_Serialize, RuleCount } from "@/bindings";
import { api, errorMessage } from "@/lib/api";
import { useFmt } from "@/composables/useFmt";
import { useWizard } from "@/stores/wizard";
import BytesDiff from "./BytesDiff.vue";
import HelpTip from "./HelpTip.vue";

const props = defineProps<{ jobId: string; rule: RuleCount; readonly?: boolean }>();
const { t, te } = useI18n();
const fmt = useFmt();
const wizard = useWizard();

const PAGE = 100;

const open = ref(props.rule.pending > 0);
const rows = ref<IssueView_Serialize[]>([]);
/** `after_id` of each visited page; the last one is the current page. */
const cursors = ref<(number | null)[]>([null]);
const hasNext = ref(false);
const loading = ref(false);
const error = ref<string | null>(null);
const busyRow = ref<number | null>(null);
const bulk = ref(false);
const editing = ref<IssueView_Serialize | null>(null);
const editName = ref("");

const title = computed(() => {
  const key = `rules.${props.rule.rule}.title`;
  return te(key) ? t(key, { n: fmt.count(props.rule.total) }, props.rule.total) : t("rules.unknown", { rule: props.rule.rule, n: fmt.count(props.rule.total) });
});
const explain = computed(() => {
  const key = `rules.${props.rule.rule}.explain`;
  return te(key) ? t(key) : "";
});
const example = computed(() => {
  const key = `rules.${props.rule.rule}.example`;
  return te(key) ? t(key) : "";
});
const showsBytes = computed(() => props.rule.rule.includes("nfc"));
/** An undecodable name has nothing to accept or keep: it can only be edited or skipped. */
const hasProposal = computed(() => props.rule.rule !== "encoding");
const pageNo = computed(() => cursors.value.length);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const after = cursors.value[cursors.value.length - 1];
    const got = await api.issues(props.jobId, props.rule.rule, after, PAGE + 1);
    hasNext.value = got.length > PAGE;
    rows.value = got.slice(0, PAGE);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    loading.value = false;
  }
}

function toggle() {
  open.value = !open.value;
  if (open.value && !rows.value.length) void load();
}
if (open.value) void load();

function next() {
  const last = rows.value[rows.value.length - 1];
  if (!last) return;
  cursors.value.push(last.id);
  void load();
}
function prev() {
  if (cursors.value.length <= 1) return;
  cursors.value.pop();
  void load();
}

const DECIDED: Record<DecisionInput["kind"], IssueDecision> = {
  accept: "accepted",
  edit: "edited",
  keep_original: "rejected",
  skip: "skipped",
};

async function decide(row: IssueView_Serialize, decision: DecisionInput) {
  busyRow.value = row.id;
  error.value = null;
  try {
    await wizard.decide(row.id, decision);
    row.decision = DECIDED[decision.kind];
    row.custom_name = decision.kind === "edit" ? decision.name : null;
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busyRow.value = null;
  }
}

function startEdit(row: IssueView_Serialize) {
  editing.value = row;
  editName.value = row.custom_name ?? row.after_text;
}
const editInvalid = computed(() => {
  const n = editName.value;
  if (!n.trim()) return t("issues.editEmpty");
  if (n.includes("/") || n.includes("\0")) return t("issues.editSlash");
  return null;
});
async function saveEdit() {
  if (!editing.value || editInvalid.value) return;
  const row = editing.value;
  editing.value = null;
  await decide(row, { kind: "edit", name: editName.value });
}

/** Applies one decision to every pending issue of this rule, in one service call. */
async function decideAllPending(decision: DecisionInput) {
  bulk.value = true;
  error.value = null;
  try {
    await wizard.decideAll(props.rule.rule, decision);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    bulk.value = false;
    if (open.value) await load();
  }
}

function decisionLabel(row: IssueView_Serialize) {
  return t(`decision.${row.decision}`);
}
</script>

<template>
  <section class="issue-group card" :class="{ pending: rule.pending > 0 }">
    <button class="group-head" :aria-expanded="open" @click="toggle">
      <AppIcon class="chev" :name="open ? 'caretDown' : 'caretRight'" />
      <span class="grow">
        <strong>{{ title }}</strong>
        <span class="muted small block">{{ explain }}</span>
        <span v-if="example" class="small block example">{{ $t("help.example") }} {{ example }}</span>
      </span>
      <span v-if="rule.pending > 0" class="badge badge-warn">{{ $t("issues.pending", { n: fmt.count(rule.pending) }) }}</span>
      <span v-else class="badge badge-ok">{{ $t("issues.resolved") }}</span>
    </button>

    <div v-if="open" class="group-body">
      <div v-if="rule.pending > 0 && !readonly" class="bulk">
        <span v-if="bulk" class="small">{{ $t("issues.bulkProgress") }}</span>
        <template v-else>
          <span class="small muted">{{ $t("issues.bulkLabel", { n: fmt.count(rule.pending) }) }}</span>
          <template v-if="hasProposal">
            <button class="btn btn-small btn-primary" :title="$t('issues.acceptAllTip')" @click="decideAllPending({ kind: 'accept' })"><AppIcon name="check" /> {{ $t("issues.acceptAll") }}</button>
            <button class="btn btn-small" :title="$t('issues.keepAllTip')" @click="decideAllPending({ kind: 'keep_original' })">{{ $t("issues.keepAll") }}</button>
          </template>
          <button class="btn btn-small" :title="$t('issues.skipAllTip')" @click="decideAllPending({ kind: 'skip' })">{{ $t("issues.skipAll") }}</button>
          <span v-if="hasProposal" class="small muted">{{ $t("issues.bulkAdvice") }}</span>
          <span v-else class="small muted">{{ $t("issues.encodingAdvice") }}</span>
        </template>
      </div>

      <p v-if="error" class="error small">{{ error }}</p>
      <p v-if="loading && !rows.length" class="muted small">{{ $t("common.loading") }}</p>

      <div v-if="rows.length" class="cols small muted" aria-hidden="true">
        <span>{{ $t("issues.colBefore") }} → {{ $t("issues.colAfter") }}</span>
        <span v-if="!readonly">{{ $t("issues.colChoose") }}</span>
      </div>
      <ol class="rows" :class="{ dim: loading }">
        <li v-for="row in rows" :key="row.id" class="row" :class="`d-${row.decision}`">
          <div class="names">
            <div class="path small muted" :title="row.path">{{ row.path }}</div>
            <div class="change">
              <span class="before" :title="row.before_text">{{ row.before_text }}</span>
              <span class="arrow" aria-hidden="true">→</span>
              <span class="after" :title="row.custom_name ?? row.after_text">
                {{ row.decision === "rejected" ? row.before_text : row.decision === "skipped" ? $t("issues.notSent") : (row.custom_name ?? row.after_text) }}
              </span>
            </div>
            <div v-if="showsBytes" class="bytes">
              <BytesDiff :before="row.before" :after="row.after" />
              <HelpTip :text="$t('issues.bytesTip')" :label="$t('issues.bytesLabel')" />
            </div>
          </div>
          <div class="state">
            <span class="chip" :class="`chip-${row.decision}`">{{ decisionLabel(row) }}</span>
          </div>
          <div v-if="!readonly" class="actions">
            <template v-if="hasProposal">
              <button class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'accepted'" :title="$t('issues.acceptTip')" @click="decide(row, { kind: 'accept' })">{{ $t("issues.accept") }}</button>
              <button class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'rejected'" :title="$t('issues.keepTip')" @click="decide(row, { kind: 'keep_original' })">{{ $t("issues.keep") }}</button>
            </template>
            <button class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'skipped'" :title="$t('issues.skipTip')" @click="decide(row, { kind: 'skip' })">{{ $t("issues.skip") }}</button>
            <button class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'edited'" :title="$t('issues.editTip')" @click="startEdit(row)">{{ $t("issues.edit") }}</button>
          </div>
        </li>
      </ol>

      <div v-if="cursors.length > 1 || hasNext" class="pager">
        <button class="btn btn-small" :disabled="cursors.length <= 1 || loading" @click="prev"><AppIcon name="back" /> {{ $t("common.previous") }}</button>
        <span class="small muted">{{ $t("issues.page", { n: pageNo }) }}</span>
        <button class="btn btn-small" :disabled="!hasNext || loading" @click="next">{{ $t("common.next") }} <AppIcon name="next" /></button>
      </div>
    </div>

    <Teleport to="body">
      <div v-if="editing" class="overlay" @click.self="editing = null" @keydown.esc="editing = null">
        <form class="card edit" role="dialog" aria-modal="true" :aria-label="$t('issues.editTitle')" @submit.prevent="saveEdit">
          <h2>{{ $t("issues.editTitle") }}</h2>
          <p class="small muted">{{ editing.path }}</p>
          <label class="field">
            <span>{{ $t("issues.editLabel") }}</span>
            <input v-model="editName" autofocus spellcheck="false" />
          </label>
          <p v-if="editInvalid" class="error small">{{ editInvalid }}</p>
          <p v-else class="small muted">{{ $t("issues.editHint") }}</p>
          <div class="dialog-actions">
            <button type="button" class="btn" @click="editing = null">{{ $t("common.cancel") }}</button>
            <button type="submit" class="btn btn-primary" :disabled="!!editInvalid">{{ $t("common.save") }}</button>
          </div>
        </form>
      </div>
    </Teleport>
  </section>
</template>

<style scoped>
.issue-group { padding: 0; overflow: hidden; }
.issue-group.pending { border-color: var(--warn); }
.group-head {
  all: unset;
  box-sizing: border-box;
  display: flex;
  gap: 0.75rem;
  align-items: center;
  width: 100%;
  padding: 0.85rem 1rem;
  cursor: pointer;
}
.group-head:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
.chev { width: 1rem; color: var(--text-2); }
.block { display: block; }
.group-body { border-top: 1px solid var(--border); padding: 0.75rem 1rem 1rem; }
.bulk { display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; margin-bottom: 0.75rem; }
.rows { list-style: none; margin: 0; padding: 0; }
.cols { display: flex; justify-content: space-between; padding-bottom: 0.3rem; border-bottom: 1px solid var(--border); }
.example { color: var(--text-2); margin-top: 0.2rem; }
.bytes { display: flex; align-items: center; gap: 0.2rem; }
.rows.dim { opacity: 0.6; }
.row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto auto;
  gap: 0.75rem;
  align-items: center;
  padding: 0.55rem 0;
  border-bottom: 1px solid var(--border);
}
.row.d-pending { background: linear-gradient(90deg, var(--warn-soft), transparent 40%); }
.names { min-width: 0; }
.path, .before, .after { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.change { display: flex; gap: 0.4rem; align-items: baseline; min-width: 0; }
.before { color: var(--text-2); max-width: 45%; }
.after { font-weight: 600; max-width: 50%; }
.arrow { color: var(--text-2); }
.actions { display: flex; gap: 0.25rem; flex-wrap: wrap; justify-content: flex-end; }
.actions [aria-pressed="true"] { border-color: var(--accent); color: var(--accent); }
.chip { font-size: 0.75rem; padding: 0.1rem 0.5rem; border-radius: 999px; background: var(--surface-2); color: var(--text-2); white-space: nowrap; }
.chip-pending { background: var(--warn-soft); color: var(--text); }
.chip-accepted, .chip-edited { background: var(--ok-soft); color: var(--text); }
.chip-skipped { background: var(--danger-soft); color: var(--text); }
.pager { display: flex; justify-content: center; align-items: center; gap: 1rem; margin-top: 0.75rem; }
.overlay { position: fixed; inset: 0; background: rgb(0 0 0 / 0.45); display: grid; place-items: center; z-index: 50; padding: 1rem; }
.edit { width: min(480px, 100%); }
.edit h2 { margin-top: 0; font-size: 1.1rem; }
.dialog-actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 1rem; }
@media (max-width: 900px) {
  .row { grid-template-columns: minmax(0, 1fr) auto; }
  .actions { grid-column: 1 / -1; justify-content: flex-start; }
}
</style>
