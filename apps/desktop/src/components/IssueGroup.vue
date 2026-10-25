<script setup lang="ts">
// One name rule's issues ("312 names with decomposed accents"). Rows are paged from SQLite with
// `after_id`, so a rule with hundreds of thousands of issues never loads at once (docs/06).
import AppIcon from "@/components/AppIcon.vue";
import { computed, inject, ref, watch } from "vue";
import SearchField from "./SearchField.vue";
import SelectionBar from "./SelectionBar.vue";
import { useI18n } from "vue-i18n";
import type { DecisionInput, IssueDecision, IssueView_Serialize, RuleCount } from "@/bindings";
import { api, errorMessage } from "@/lib/api";
import { useFmt } from "@/composables/useFmt";
import { DECISIONS, type Decisions } from "@/lib/decisions";
import { useWizard } from "@/stores/wizard";
import BytesDiff from "./BytesDiff.vue";
import HelpTip from "./HelpTip.vue";
import ModalDialog from "./ModalDialog.vue";

const props = defineProps<{
  jobId: string;
  rule: RuleCount;
  readonly?: boolean;
  /** Whether the list starts open; default: while it has pending names. */
  startOpen?: boolean;
}>();
const { t, te } = useI18n();
const fmt = useFmt();
/** The wizard's job, unless the page provides its own (a Repair job). */
const wizard: Decisions = inject(DECISIONS, null) ?? useWizard();

const PAGE = 100;

const open = ref(props.startOpen ?? props.rule.pending > 0);
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
/** Path filter of this group (docs/design/specs/D08). */
const query = ref("");

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
/** NFC changes are invisible as text, and a decoded name is a reading of raw bytes: show them. */
const showsBytes = computed(() => props.rule.rule.includes("nfc") || props.rule.rule === "encoding");
/** A row whose proposal differs from its name: an undecodable one has none to accept. */
function rowHasProposal(row: IssueView_Serialize): boolean {
  return row.after.length !== row.before.length || row.after.some((b, i) => b !== row.before[i]);
}
/** "Accept all" applies to every pending issue of the rule, loaded or not: only when none of them
 *  lacks a proposal (the service refuses the whole lot otherwise). */
const canAcceptAll = computed(() => props.rule.no_proposal === 0);
/** Accepting a selection works for the issues that have a proposal; the others are reported. */
const hasProposal = computed(() => props.rule.no_proposal < props.rule.pending || rows.value.some(rowHasProposal));
/** Bytes that aren't UTF-8 can't be kept: the destination takes UTF-8 names only. */
const canKeep = computed(() => props.rule.rule !== "encoding");
const pageNo = computed(() => cursors.value.length);

async function load() {
  loading.value = true;
  error.value = null;
  try {
    const after = cursors.value[cursors.value.length - 1];
    const got = await api.issues(props.jobId, props.rule.rule, after, PAGE + 1, query.value.trim() || null);
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

// ---- filter and selection (docs/design/specs/D08) ----
/** Pending issues of this rule whose path matches the filter (for "Select the N that match"). */
const matchingPending = ref<number | null>(null);
watch(query, async (q) => {
  cursors.value = [null];
  clearSelection();
  await load();
  matchingPending.value = null;
  if (!q.trim()) return;
  try {
    const counts = await api.ruleCounts(props.jobId, q.trim());
    matchingPending.value = counts.find((r) => r.rule === props.rule.rule)?.pending ?? 0;
  } catch {
    matchingPending.value = null;
  }
});

const selected = ref<Set<number>>(new Set());
/** Every pending issue that matches the filter, including pages not loaded: kept as a query. */
const selectAllMatching = ref(false);
const lastClicked = ref<number | null>(null);
const notice = ref<string | null>(null);
const selectedCount = computed(() => (selectAllMatching.value ? (matchingPending.value ?? props.rule.pending) : selected.value.size));

function claimGroup() {
  if (wizard.selectionGroup && wizard.selectionGroup !== props.rule.rule) notice.value = null;
  wizard.selectionGroup = props.rule.rule;
}
// Selecting in another group clears this one, and says so.
watch(
  () => wizard.selectionGroup,
  (g) => {
    if (g !== props.rule.rule && (selected.value.size || selectAllMatching.value)) {
      clearSelection();
      notice.value = t("issues.oneGroup");
    }
  },
);
function clearSelection() {
  selected.value = new Set();
  selectAllMatching.value = false;
}
function toggleRow(row: IssueView_Serialize, index: number, e: MouseEvent | KeyboardEvent) {
  claimGroup();
  selectAllMatching.value = false;
  const next = new Set(selected.value);
  if ("shiftKey" in e && e.shiftKey && lastClicked.value != null) {
    const [a, b] = [Math.min(lastClicked.value, index), Math.max(lastClicked.value, index)];
    for (const r of rows.value.slice(a, b + 1)) next.add(r.id);
  } else if (next.has(row.id)) next.delete(row.id);
  else next.add(row.id);
  lastClicked.value = index;
  selected.value = next;
}
const pageAllSelected = computed(() => rows.value.length > 0 && rows.value.every((r) => selected.value.has(r.id)));
function togglePage() {
  claimGroup();
  selectAllMatching.value = false;
  const next = new Set(selected.value);
  if (pageAllSelected.value) for (const r of rows.value) next.delete(r.id);
  else for (const r of rows.value) next.add(r.id);
  selected.value = next;
}
function selectMatching() {
  claimGroup();
  selected.value = new Set();
  selectAllMatching.value = true;
}

const selectionActions = computed(() => [
  ...(hasProposal.value ? [{ id: "accept", label: t("issues.accept"), primary: true }] : []),
  ...(canKeep.value ? [{ id: "keep_original", label: t("issues.keep") }] : []),
  { id: "skip", label: t("issues.skip") },
]);
async function applySelection(kind: string) {
  const decision = { kind } as DecisionInput;
  bulk.value = true;
  error.value = null;
  notice.value = null;
  try {
    if (selectAllMatching.value) {
      const n = await wizard.decideAll(props.rule.rule, decision, query.value.trim() || null);
      notice.value = t(`issues.applied.${kind}`, { n: fmt.count(n) }, n);
    } else {
      const res = await wizard.decideMany([...selected.value], decision);
      notice.value = t(`issues.applied.${kind}`, { n: fmt.count(res.changed) }, res.changed);
      if (res.refused.length) error.value = t("issues.partial", { n: res.refused.length, reason: res.refused[0].reason }, res.refused.length);
    }
    clearSelection();
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    bulk.value = false;
    await load();
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
      <strong class="g-title">{{ title }}</strong>
      <span v-if="rule.pending > 0" class="badge badge-warn g-state">{{ $t("issues.pending", { n: fmt.count(rule.pending) }) }}</span>
      <span v-else class="badge badge-ok g-state">{{ $t("issues.resolved") }}</span>
      <span class="g-text">
        <span class="muted small block">{{ explain }}</span>
        <span v-if="example" class="small block example">{{ $t("help.example") }} {{ example }}</span>
      </span>
    </button>

    <div v-if="open" class="group-body">
      <div v-if="rule.pending > 0 && !readonly" class="bulk">
        <span v-if="bulk" class="small">{{ $t("issues.bulkProgress") }}</span>
        <template v-else>
          <span class="small muted">{{ $t("issues.bulkLabel", { n: fmt.count(rule.pending) }) }}</span>
          <button v-if="canAcceptAll" class="btn btn-small btn-primary" :title="$t('issues.acceptAllTip')" @click="decideAllPending({ kind: 'accept' })"><AppIcon name="check" /> {{ $t("issues.acceptAll") }}</button>
          <button v-if="canKeep" class="btn btn-small" :title="$t('issues.keepAllTip')" @click="decideAllPending({ kind: 'keep_original' })">{{ $t("issues.keepAll") }}</button>
          <button class="btn btn-small" :title="$t('issues.skipAllTip')" @click="decideAllPending({ kind: 'skip' })">{{ $t("issues.skipAll") }}</button>
          <span v-if="canKeep" class="small muted">{{ $t("issues.bulkAdvice") }}</span>
          <span v-else class="small muted">{{ $t("issues.encodingAdvice") }}</span>
        </template>
      </div>

      <div v-if="!readonly" class="select-tools">
        <SearchField v-model="query" :placeholder="$t('issues.filter')" />
        <label v-if="rows.length" class="check small">
          <input type="checkbox" :checked="pageAllSelected" @change="togglePage" />
          <span>{{ $t("issues.selectPage") }}</span>
        </label>
        <button v-if="query.trim() && matchingPending" class="link small" @click="selectMatching">
          {{ $t("issues.selectMatching", { n: fmt.count(matchingPending) }, matchingPending) }}
        </button>
      </div>

      <p v-if="notice" class="small ok-text" role="status">{{ notice }}</p>
      <p v-if="error" class="error small">{{ error }}</p>
      <p v-if="loading && !rows.length" class="muted small">{{ $t("common.loading") }}</p>

      <div v-if="rows.length" class="cols small muted" aria-hidden="true">
        <span>{{ $t("issues.colBefore") }} → {{ $t("issues.colAfter") }}</span>
        <span v-if="!readonly">{{ $t("issues.colChoose") }}</span>
      </div>
      <ol class="rows" :class="{ dim: loading }">
        <li v-for="(row, index) in rows" :key="row.id" class="row" :class="[`d-${row.decision}`, { selectable: !readonly, selected: selected.has(row.id) || selectAllMatching }]">
          <input
            v-if="!readonly"
            type="checkbox"
            class="pick"
            :checked="selected.has(row.id) || (selectAllMatching && row.decision === 'pending')"
            :aria-label="$t('issues.selectRow', { path: row.path })"
            @click="toggleRow(row, index, $event)"
          />
          <div class="names">
            <div class="path small muted" :title="row.path">{{ row.path }}</div>
            <div class="change">
              <span class="before" :title="row.before_text">{{ row.before_text }}</span>
              <span class="arrow" aria-hidden="true">→</span>
              <span class="after" :title="row.custom_name ?? row.after_text">
                {{ row.decision === "rejected" ? row.before_text : row.decision === "skipped" ? $t("issues.notSent") : (row.custom_name ?? row.after_text) }}
              </span>
            </div>
            <div v-if="row.rule === 'collision' && row.decision === 'rejected'" class="small warn-text">{{ $t("issues.keptClash", { name: row.before_text }) }}</div>
            <div v-if="showsBytes" class="bytes">
              <BytesDiff :before="row.before" :after="row.after" />
              <HelpTip :text="$t('issues.bytesTip')" :label="$t('issues.bytesLabel')" />
            </div>
          </div>
          <div class="state">
            <!-- The chosen button says the rest; a word only while nothing is chosen. -->
            <span v-if="row.decision === 'pending' || readonly" class="chip" :class="`chip-${row.decision}`">{{ decisionLabel(row) }}</span>
            <span v-if="row.carried_over" class="badge badge-muted" :title="$t('issues.carriedTip')"><AppIcon name="again" :size="12" /> {{ $t("issues.carried") }}</span>
          </div>
          <div v-if="!readonly" class="actions">
            <div class="seg" role="group" :aria-label="$t('issues.colChoose')">
            <button v-if="rowHasProposal(row)" class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'accepted'" :title="$t('issues.acceptTip')" @click="decide(row, { kind: 'accept' })">{{ $t("issues.accept") }}</button>
            <button v-if="canKeep" class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'rejected'" :title="$t('issues.keepTip')" @click="decide(row, { kind: 'keep_original' })">{{ $t("issues.keep") }}</button>
            <button class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'skipped'" :title="$t('issues.skipTip')" @click="decide(row, { kind: 'skip' })">{{ $t("issues.skip") }}</button>
            </div>
            <button class="btn btn-small edit-btn" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'edited'" :title="$t('issues.editTip')" @click="startEdit(row)">{{ $t("issues.edit") }}</button>
          </div>
        </li>
      </ol>

      <SelectionBar :count="selectedCount" :busy="bulk" :actions="selectionActions" @act="applySelection" @clear="clearSelection" />

      <div v-if="cursors.length > 1 || hasNext" class="pager">
        <button class="btn btn-small" :disabled="cursors.length <= 1 || loading" @click="prev"><AppIcon name="back" /> {{ $t("common.previous") }}</button>
        <span class="small muted">{{ $t("issues.page", { n: pageNo }) }}</span>
        <button class="btn btn-small" :disabled="!hasNext || loading" @click="next">{{ $t("common.next") }} <AppIcon name="next" /></button>
      </div>
    </div>

    <ModalDialog :open="!!editing" :title="$t('issues.editTitle')" @close="editing = null">
      <form v-if="editing" class="edit" @submit.prevent="saveEdit">
        <p class="small muted">{{ editing.path }}</p>
        <label class="field">
          <span>{{ $t("issues.editLabel") }}</span>
          <input v-model="editName" data-autofocus spellcheck="false" />
        </label>
        <p v-if="editInvalid" class="error small">{{ editInvalid }}</p>
        <p v-else class="small muted">{{ $t("issues.editHint") }}</p>
        <div class="dialog-actions">
          <button type="button" class="btn" @click="editing = null">{{ $t("common.cancel") }}</button>
          <button type="submit" class="btn btn-primary" :disabled="!!editInvalid">{{ $t("common.save") }}</button>
        </div>
      </form>
    </ModalDialog>
  </section>
</template>

<style scoped>
.issue-group { padding: 0; overflow: hidden; }
.issue-group.pending { border-color: var(--warn); }
.group-head {
  all: unset;
  box-sizing: border-box;
  /* The title and its state on the first line, the explanation under both with the whole width
     (beside the badge it was a narrow column of many lines on a phone). */
  display: grid;
  grid-template-columns: 1rem minmax(0, 1fr) auto;
  grid-template-areas: "chev title state" ". text text";
  gap: 0.2rem 0.75rem;
  align-items: center;
  width: 100%;
  padding: 0.85rem 1rem;
  cursor: pointer;
}
.chev { grid-area: chev; }
.g-title { grid-area: title; }
.g-state { grid-area: state; }
.g-text { grid-area: text; }
.group-head:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
.chev { width: 1rem; color: var(--text-2); }
.block { display: block; }
.group-body { border-top: 1px solid var(--border); padding: 0.75rem 1rem 1rem; container-type: inline-size; }
.bulk { display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; margin-bottom: 0.75rem; }
.rows { list-style: none; margin: 0; padding: 0; }
.cols { display: flex; justify-content: space-between; padding-bottom: 0.3rem; border-bottom: 1px solid var(--border); }
.example { color: var(--text-2); margin-top: 0.2rem; }
.bytes { display: flex; align-items: center; gap: 0.2rem; }
.bytes > :first-child { min-width: 0; }
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
.row.selectable { grid-template-columns: auto minmax(0, 1fr) auto auto; }
.row.selected { background: var(--accent-soft); }
.pick { width: 1rem; height: 1rem; }
.select-tools { display: flex; gap: 0.8rem; align-items: center; flex-wrap: wrap; margin-bottom: 0.6rem; }
.check { display: inline-flex; gap: 0.35rem; align-items: center; cursor: pointer; }
.link { all: unset; cursor: pointer; color: var(--accent); }
.link:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.names { min-width: 0; }
.path, .before, .after { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.change { display: flex; gap: 0.4rem; align-items: baseline; min-width: 0; }
.before { color: var(--text-2); max-width: 45%; }
.after { font-weight: 600; max-width: 50%; }
.arrow { color: var(--text-2); }
.actions { display: flex; gap: 0.25rem; flex-wrap: wrap; justify-content: flex-end; }
.actions { align-items: center; }
/* Accept · Keep · Skip as one control: the chosen one filled (docs/04, "Decisions model"). */
.seg { display: inline-flex; }
.seg .btn { border-radius: 0; margin-left: -1px; }
.seg .btn:first-child { border-radius: var(--radius-sm) 0 0 var(--radius-sm); margin-left: 0; }
.seg .btn:last-child { border-radius: 0 var(--radius-sm) var(--radius-sm) 0; }
.seg .btn:only-child { border-radius: var(--radius-sm); }
.seg .btn[aria-pressed="true"] { background: var(--accent-soft); border-color: var(--accent); color: var(--accent); font-weight: 600; position: relative; z-index: 1; }
.edit-btn[aria-pressed="true"] { background: var(--accent-soft); border-color: var(--accent); color: var(--accent); font-weight: 600; }
.chip { font-size: var(--fs-xs); padding: 0.1rem 0.5rem; border-radius: var(--radius-pill); background: var(--surface-2); color: var(--text-2); white-space: nowrap; }
.chip-pending { background: var(--warn-soft); color: var(--text); }
.chip-accepted, .chip-edited { background: var(--ok-soft); color: var(--text); }
.chip-skipped { background: var(--danger-soft); color: var(--text); }
.pager { display: flex; justify-content: center; align-items: center; gap: 1rem; margin-top: 0.75rem; }
.edit { display: grid; gap: var(--sp-2); }
.edit p { margin: 0; }
.dialog-actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 1rem; }
/* The buttons go under the names when the row can't hold them beside. */
@container (max-width: 680px) {
  .row, .row.selectable { grid-template-columns: auto minmax(0, 1fr) auto; }
  .row:not(.selectable) { grid-template-columns: minmax(0, 1fr) auto; }
  .actions { grid-column: 1 / -1; justify-content: flex-start; }
  .cols > span + span { display: none; }
}
/* Narrower still: the name at the destination gets its own line. */
@container (max-width: 480px) {
  .change { flex-wrap: wrap; row-gap: 0; }
  .before { max-width: calc(100% - 1.5rem); }
  .after { flex-basis: 100%; max-width: 100%; }
}
</style>
