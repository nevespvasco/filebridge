<script setup lang="ts">
// A stacked bar with a legend (docs/design/01-design-system.md, "Components"). `composition`:
// the segments are the whole. `usage`: what the segments don't fill is free space. The bar is
// decoration; the legend and a hidden table carry the numbers.
import { computed } from "vue";
import { useFmt } from "@/composables/useFmt";

export type Segment = { key: string; label: string; bytes: number; color: string; files?: number };

const props = withDefaults(
  defineProps<{
    segments: Segment[];
    /** The bar's full width in bytes; defaults to the sum of the segments. */
    total?: number | null;
    mode?: "usage" | "composition";
    freeLabel?: string;
    caption: string;
    /** Bytes beyond `total` (does not fit): drawn as a danger cap at the end. */
    overflow?: boolean;
  }>(),
  { total: null, mode: "composition", freeLabel: "", overflow: false },
);
const fmt = useFmt();

const sum = computed(() => props.segments.reduce((n, s) => n + s.bytes, 0));
const whole = computed(() => Math.max(props.total ?? sum.value, 1));
const free = computed(() => (props.mode === "usage" ? Math.max(whole.value - sum.value, 0) : 0));
/** Widths in %, with a minimum so a tiny segment stays visible. */
const widths = computed(() => props.segments.map((s) => (s.bytes > 0 ? Math.max((s.bytes / whole.value) * 100, 0.6) : 0)));
const pct = (b: number) => (b / whole.value) * 100;
</script>

<template>
  <figure class="meter">
    <div class="bar" :class="{ overflow }" aria-hidden="true">
      <span v-for="(s, i) in segments" :key="s.key" class="seg" :style="{ width: `${Math.min(widths[i], 100)}%`, background: s.color }" />
    </div>
    <ul class="legend small">
      <li v-for="s in segments" :key="s.key">
        <span class="swatch" :style="{ background: s.color }" aria-hidden="true" />
        <span>{{ s.label }}</span>
        <span class="num muted">
          <template v-if="s.files != null">{{ fmt.count(s.files) }} · </template>{{ fmt.bytes(s.bytes) }}
        </span>
      </li>
      <li v-if="mode === 'usage' && freeLabel">
        <span class="swatch free" aria-hidden="true" />
        <span>{{ freeLabel }}</span>
        <span class="num muted">{{ fmt.bytes(free) }}</span>
      </li>
    </ul>
    <table class="sr-only">
      <caption>{{ caption }}</caption>
      <tr v-for="s in segments" :key="s.key">
        <th scope="row">{{ s.label }}</th>
        <td>{{ fmt.bytes(s.bytes) }}</td>
        <td>{{ Math.round(pct(s.bytes)) }} %</td>
      </tr>
    </table>
  </figure>
</template>

<style scoped>
.meter { margin: 0; display: grid; gap: 0.5rem; }
.bar { display: flex; height: 12px; border-radius: 6px; overflow: hidden; background: var(--track); }
.bar.overflow { box-shadow: inset -4px 0 0 var(--danger); }
.seg { height: 100%; transition: width 0.3s ease; }
.seg + .seg { box-shadow: inset 1px 0 0 var(--surface); }
.legend { list-style: none; display: flex; flex-wrap: wrap; gap: 0.3rem 1.2rem; margin: 0; padding: 0; }
.legend li { display: inline-flex; gap: 0.4rem; align-items: center; }
.swatch { width: 10px; height: 10px; border-radius: 3px; flex: none; }
.swatch.free { background: var(--track); box-shadow: inset 0 0 0 1px var(--border-strong); }
</style>
