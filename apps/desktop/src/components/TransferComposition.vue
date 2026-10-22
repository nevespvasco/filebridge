<script setup lang="ts">
// What a transfer contains, by file Category, and whether it fits at the destination
// (docs/design/specs/D02-source-and-composition.md).
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { CategoryCount } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { api } from "@/lib/api";
import { categoryColor } from "@/lib/categories";
import StorageMeter, { type Segment } from "./StorageMeter.vue";

const props = defineProps<{
  jobId: string;
  /** Destination space, when known (free from the preflight; total and used from a storage check). */
  free?: number | null;
  total?: number | null;
  used?: number | null;
  destination?: string;
}>();
const { t } = useI18n();
const fmt = useFmt();

const cats = ref<CategoryCount[]>([]);
const loaded = ref(false);
watch(
  () => props.jobId,
  async (id) => {
    loaded.value = false;
    try {
      cats.value = await api.jobCategories(id);
    } catch {
      cats.value = [];
    } finally {
      loaded.value = true;
    }
  },
  { immediate: true },
);

const files = computed(() => cats.value.reduce((n, c) => n + c.files, 0));
const bytes = computed(() => cats.value.reduce((n, c) => n + c.bytes, 0));
/** Jobs analysed before Categories existed have every file as Other. */
const unknown = computed(() => cats.value.length === 1 && cats.value[0].category === "other" && files.value > 20);
const composition = computed<Segment[]>(() =>
  cats.value.map((c) => ({ key: c.category, label: t(`category.${c.category}`), bytes: c.bytes, files: c.files, color: categoryColor(c.category) })),
);

/** The tighter of what is known: total = used + free when only those two are. */
const knownTotal = computed(() => props.total ?? (props.used != null && props.free != null ? props.used + props.free : null));
const fits = computed(() => (props.free == null ? null : bytes.value <= props.free));
const freeAfter = computed(() => (props.free == null ? null : props.free - bytes.value));
const tight = computed(() => {
  if (freeAfter.value == null || freeAfter.value < 0) return false;
  return freeAfter.value < 10e9 || (knownTotal.value != null && freeAfter.value < knownTotal.value * 0.05);
});
const fit = computed<Segment[]>(() => {
  const out: Segment[] = [];
  if (props.used != null) out.push({ key: "used", label: t("composition.used"), bytes: props.used, color: "var(--text-2)" });
  out.push({ key: "this", label: t("composition.this"), bytes: bytes.value, color: "var(--info)" });
  return out;
});
/** Without a total, the bar compares the transfer with the free space. */
const fitWhole = computed(() => knownTotal.value ?? (props.free != null ? Math.max(props.free, bytes.value) : null));
</script>

<template>
  <section v-if="loaded && files" class="card composition" :aria-label="$t('composition.title')">
    <header class="head">
      <h3>{{ $t("composition.title") }}</h3>
      <span class="small muted num">{{ $t("composition.totals", { files: fmt.count(files), size: fmt.bytes(bytes) }) }}</span>
    </header>
    <p v-if="unknown" class="small muted">{{ $t("composition.unknown") }}</p>
    <StorageMeter v-else :segments="composition" :caption="$t('composition.title')" />

    <template v-if="free != null">
      <header class="head fit">
        <h3>{{ $t("composition.fit") }}</h3>
        <span v-if="destination" class="small muted">{{ destination }}</span>
      </header>
      <StorageMeter
        :segments="fit"
        :total="fitWhole"
        mode="usage"
        :free-label="$t('composition.freeAfter')"
        :caption="$t('composition.fit')"
        :overflow="fits === false"
      />
      <p v-if="knownTotal == null" class="small muted">{{ $t("composition.noTotal") }}</p>
      <p v-if="fits === false" class="small error">{{ $t("analysis.tooBig", { free: fmt.bytes(free), size: fmt.bytes(bytes) }) }}</p>
      <p v-else-if="tight" class="guide warn small">{{ $t("composition.tight", { destination: destination ?? "", free: fmt.bytes(freeAfter) }) }}</p>
    </template>
    <p v-else class="small muted">{{ $t("composition.noFree") }}</p>
  </section>
</template>

<style scoped>
.composition { display: grid; gap: 0.6rem; margin: 1rem 0; }
.head { display: flex; justify-content: space-between; align-items: baseline; gap: 1rem; flex-wrap: wrap; }
.head h3 { font-size: 0.95rem; margin: 0; }
.head.fit { margin-top: 0.5rem; }
.guide { margin: 0; }
</style>
