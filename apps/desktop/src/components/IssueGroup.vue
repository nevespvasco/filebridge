<script setup lang="ts">
// One name rule's issues ("312 names with decomposed accents"). Rows are paged from SQLite with
// `after_id`, so a rule with hundreds of thousands of issues never loads at once (docs/06).
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { DecisionInput, IssueView_Serialize, RuleCount } from "@/bindings";
import { api, errorMessage } from "@/lib/api";
import { useFmt } from "@/composables/useFmt";
import { useWizard } from "@/stores/wizard";
import BytesDiff from "./BytesDiff.vue";

const props = defineProps<{ jobId: string; rule: RuleCount; readonly?: boolean }>();
const { t, te } = useI18n();
const fmt = useFmt();
const wizard = useWizard();

const PAGE = 100;
const BULK_PAGE = 1000;

const open = ref(props.rule.pending > 0);
const rows = ref<IssueView_Serialize[]>([]);
/** `after_id` of each visited page; the last one is the current page. */
const cursors = ref<(number | null)[]>([null]);
const hasNext = ref(false);
const loading = ref(false);
const error = ref<string | null>(null);
const busyRow = ref<number | null>(null);
const bulk = ref<{ done: number; total: number; stop: boolean } | null>(null);
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
const showsBytes = computed(() => props.rule.rule.includes("nfc"));
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

const DECIDED: Record<DecisionInput["kind"], string> = {
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

/** Applies one decision to every pending issue of this rule, page by page. */
async function decideAllPending(decision: DecisionInput) {
  bulk.value = { done: 0, total: props.rule.pending, stop: false };
  error.value = null;
  try {
    let after: number | null = null;
    for (;;) {
      const page: IssueView_Serialize[] = await api.issues(props.jobId, props.rule.rule, after, BULK_PAGE);
      if (!page.length) break;
      for (const issue of page) {
        if (bulk.value.stop) return;
        if (issue.decision !== "pending") continue;
        await api.decide(props.jobId, issue.id, decision);
        bulk.value.done++;
      }
      after = page[page.length - 1].id;
      if (page.length < BULK_PAGE) break;
    }
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    bulk.value = null;
    await wizard.refreshRuleCounts();
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
      <span class="chev" aria-hidden="true">{{ open ? "▾" : "▸" }}</span>
      <span class="grow">
        <strong>{{ title }}</strong>
        <span class="muted small block">{{ explain }}</span>
      </span>
      <span v-if="rule.pending > 0" class="badge badge-warn">{{ $t("issues.pending", { n: fmt.count(rule.pending) }) }}</span>
      <span v-else class="badge badge-ok">{{ $t("issues.resolved") }}</span>
    </button>

    <div v-if="open" class="group-body">
      <div v-if="rule.pending > 0 && !readonly" class="bulk">
        <template v-if="bulk">
          <span class="small">{{ $t("issues.bulkProgress", { done: fmt.count(bulk.done), total: fmt.count(bulk.total) }) }}</span>
          <button class="btn btn-small" @click="bulk.stop = true">{{ $t("common.stop") }}</button>
        </template>
        <template v-else>
          <span class="small muted">{{ $t("issues.bulkLabel", { n: fmt.count(rule.pending) }) }}</span>
          <button class="btn btn-small" @click="decideAllPending({ kind: 'accept' })">{{ $t("issues.acceptAll") }}</button>
          <button class="btn btn-small" @click="decideAllPending({ kind: 'keep_original' })">{{ $t("issues.keepAll") }}</button>
          <button class="btn btn-small" @click="decideAllPending({ kind: 'skip' })">{{ $t("issues.skipAll") }}</button>
        </template>
      </div>

      <p v-if="error" class="error small">{{ error }}</p>
      <p v-if="loading && !rows.length" class="muted small">{{ $t("common.loading") }}</p>

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
            <BytesDiff v-if="showsBytes" :before="row.before" :after="row.after" />
          </div>
          <div class="state">
            <span class="chip" :class="`chip-${row.decision}`">{{ decisionLabel(row) }}</span>
          </div>
          <div v-if="!readonly" class="actions">
            <button class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'accepted'" @click="decide(row, { kind: 'accept' })">{{ $t("issues.accept") }}</button>
            <button class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'rejected'" @click="decide(row, { kind: 'keep_original' })">{{ $t("issues.keep") }}</button>
            <button class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'skipped'" @click="decide(row, { kind: 'skip' })">{{ $t("issues.skip") }}</button>
            <button class="btn btn-small" :disabled="busyRow === row.id || !!bulk" :aria-pressed="row.decision === 'edited'" @click="startEdit(row)">{{ $t("issues.edit") }}</button>
          </div>
        </li>
      </ol>

      <div v-if="cursors.length > 1 || hasNext" class="pager">
        <button class="btn btn-small" :disabled="cursors.length <= 1 || loading" @click="prev">← {{ $t("common.previous") }}</button>
        <span class="small muted">{{ $t("issues.page", { n: pageNo }) }}</span>
        <button class="btn btn-small" :disabled="!hasNext || loading" @click="next">{{ $t("common.next") }} →</button>
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
