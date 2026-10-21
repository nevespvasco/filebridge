<script setup lang="ts">
// The bytes of a name before and after a fix. For NFC changes the text looks identical, so the
// bytes are what shows the difference (e.g. `61 cc 81` → `c3 a1` for "á").
import { computed } from "vue";

const props = defineProps<{ before: number[]; after: number[] }>();

type Token = { text: string; kind: "ctx" | "del" | "ins" };

/** Trims the common prefix and suffix so only the changed bytes (plus a little context) show. */
const rows = computed(() => {
  const a = props.before;
  const b = props.after;
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const ctx = 2;
  const from = Math.max(0, start - ctx);
  const hex = (arr: number[], lo: number, hi: number) =>
    arr.slice(lo, hi).map((x) => x.toString(16).padStart(2, "0"));
  const row = (arr: number[], end: number, kind: "del" | "ins"): Token[] => {
    const changed = hex(arr, start, end).join(" ") || "∅";
    const tail = Math.min(arr.length, end + ctx);
    return [
      ...(from > 0 ? [{ text: "…", kind: "ctx" as const }] : []),
      ...hex(arr, from, start).map((text) => ({ text, kind: "ctx" as const })),
      { text: changed, kind },
      ...hex(arr, end, tail).map((text) => ({ text, kind: "ctx" as const })),
      ...(tail < arr.length ? [{ text: "…", kind: "ctx" as const }] : []),
    ];
  };
  return [row(a, endA, "del"), row(b, endB, "ins")];
});
</script>

<template>
  <div class="bytes-diff small mono">
    <span class="row">
      <span v-for="(tok, i) in rows[0]" :key="i" :class="tok.kind">{{ tok.text }}</span>
    </span>
    <span class="arrow" aria-hidden="true">→</span>
    <span class="row">
      <span v-for="(tok, i) in rows[1]" :key="i" :class="tok.kind">{{ tok.text }}</span>
    </span>
  </div>
</template>

<style scoped>
.bytes-diff { display: flex; gap: 0.5rem; align-items: center; flex-wrap: wrap; }
.row { display: inline-flex; gap: 0.35em; }
.ctx { color: var(--text-2); }
.del, .ins { border-radius: 3px; padding: 0 2px; }
.del { background: var(--danger-soft); }
.ins { background: var(--ok-soft); }
.arrow { color: var(--text-2); }
</style>