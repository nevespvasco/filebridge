<script setup lang="ts">
// The chosen sources (docs/design/specs/D02): what each one is, what is wrong with it and the
// one-click fix. Removing from this list never touches the files.
import { computed, nextTick, ref } from "vue";
import { SOURCE_CHILDREN_CAP, type SourceInfo_Serialize, type SourceProblem } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import { CATEGORY_ICON } from "@/lib/categories";
import { baseName, parentPath } from "@/lib/format";
import type { IconName } from "@/lib/icons";
import AppIcon from "./AppIcon.vue";
import RelativeTime from "./RelativeTime.vue";

const props = defineProps<{
  sources: string[];
  info: Record<string, SourceInfo_Serialize>;
  /** Path → the chosen source that already contains it. */
  coveredBy: Record<string, string>;
}>();
const emit = defineEmits<{ remove: [path: string]; replace: [old: string, next: string] }>();
const fmt = useFmt();

const PAGE = 100;
const limit = ref(PAGE);
const shown = computed(() => props.sources.slice(0, limit.value));
const list = ref<HTMLElement | null>(null);

const PROBLEM_ICON: Record<SourceProblem, IconName> = {
  missing: "missing",
  unreadable: "lock",
  symlink: "link",
  root: "warning",
  special: "warning",
};

function icon(path: string): IconName {
  const i = props.info[path];
  if (i?.problem) return PROBLEM_ICON[i.problem];
  if (!i || i.is_dir) return "folder";
  return CATEGORY_ICON[i.category ?? "other"];
}
function covered(path: string): string | null {
  const c = props.coveredBy[path];
  return c && props.sources.includes(c) ? c : null;
}

/** Keeps the keyboard in the list: focus goes to the row that took this one's place. */
async function remove(path: string, i: number) {
  emit("remove", path);
  await nextTick();
  const rows = list.value?.querySelectorAll<HTMLElement>(".remove");
  rows?.[Math.min(i, rows.length - 1)]?.focus();
}
</script>

<template>
  <ul ref="list" class="sources">
    <li v-for="(p, i) in shown" :key="p" :class="{ bad: !!info[p]?.problem || !!covered(p) }">
      <span class="icon" :class="{ warn: !!info[p]?.problem || !!covered(p) }"><AppIcon :name="icon(p)" :size="22" /></span>
      <div class="what">
        <div class="name-line">
          <span class="name" :title="p">{{ baseName(p) || p }}</span>
          <!-- rtl so a long path loses its start, not its end; <bdi> keeps the text itself ltr. -->
          <span class="parent small muted" :title="p"><bdi>{{ parentPath(p) }}</bdi></span>
        </div>

        <p v-if="info[p]?.problem" class="small problem">
          {{ $t(`source.problem.${info[p].problem}`, { target: info[p].link_target ?? "" }) }}
          <button
            v-if="info[p].problem === 'symlink' && info[p].link_target"
            class="link"
            type="button"
            @click="emit('replace', p, info[p].link_target!)"
          >{{ $t("source.useTarget", { name: baseName(info[p].link_target!) }) }}</button>
          <button v-else class="link" type="button" @click="remove(p, i)">{{ $t("source.removeIt") }}</button>
        </p>
        <p v-else-if="covered(p)" class="small problem">
          {{ $t("source.covered", { name: baseName(covered(p)!) }) }}
          <button class="link" type="button" @click="remove(p, i)">{{ $t("source.removeIt") }}</button>
        </p>
        <p v-else-if="info[p]" class="small muted details">
          <template v-if="info[p].is_dir">
            <span v-if="info[p].children === 0">{{ $t("source.emptyFolder") }}</span>
            <span v-else-if="info[p].children_capped">{{ $t("source.itemsCapped", { n: fmt.count(SOURCE_CHILDREN_CAP) }) }}</span>
            <span v-else>{{ $t("source.items", { n: fmt.count(info[p].children) }, info[p].children ?? 0) }}</span>
          </template>
          <template v-else>
            <span class="num">{{ fmt.bytes(info[p].size) }}</span>
            <span v-if="info[p].category"> · {{ $t(`category.${info[p].category}`) }}</span>
          </template>
          <span v-if="info[p].mtime"> · {{ $t("source.changed") }} <RelativeTime :at="info[p].mtime!" /></span>
        </p>
        <p v-else class="small muted details"><span class="skeleton" />{{ $t("source.checking") }}</p>
      </div>
      <button
        class="btn btn-small remove"
        type="button"
        :title="$t('source.removeTip')"
        :aria-label="$t('source.remove', { path: p })"
        @click="remove(p, i)"
      ><AppIcon name="close" /></button>
    </li>
    <li v-if="sources.length > limit" class="more">
      <button class="link" type="button" @click="limit += PAGE">
        {{ $t("source.showMore", { n: fmt.count(Math.min(PAGE, sources.length - limit)) }) }}
      </button>
    </li>
  </ul>
</template>

<style scoped>
.sources { list-style: none; margin: 0; padding: 0; }
.sources li {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto;
  gap: 0.75rem;
  align-items: center;
  padding: 0.6rem 0.25rem;
  border-bottom: 1px solid var(--rule);
}
.sources li:last-child { border-bottom: 0; }
.sources li.bad { background: color-mix(in srgb, var(--warn-soft) 55%, transparent); border-radius: var(--radius-sm); }
.icon { display: grid; place-items: center; width: 2.4rem; height: 2.4rem; border-radius: var(--radius-sm); background: var(--accent-soft); color: var(--accent); }
.icon.warn { background: var(--warn-soft); color: var(--warn-strong); }
.what { min-width: 0; display: grid; gap: 0.15rem; }
.name-line { display: flex; gap: 0.5rem; align-items: baseline; min-width: 0; }
.name { font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; flex: none; max-width: 60%; }
.parent { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; direction: rtl; text-align: left; }
.what p { margin: 0; }
.problem { color: var(--warn-strong); }
.link { all: unset; cursor: pointer; color: var(--accent); font-weight: 600; margin-left: 0.35rem; }
.link:hover { text-decoration: underline; }
.link:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; border-radius: 2px; }
.more { display: block !important; text-align: center; }
.more .link { margin: 0; }
.skeleton {
  display: inline-block;
  width: 5rem;
  height: 0.7em;
  margin-right: 0.5rem;
  border-radius: 4px;
  background: linear-gradient(90deg, var(--surface-2), var(--border), var(--surface-2));
  background-size: 200% 100%;
  animation: shimmer 1.2s linear infinite;
  vertical-align: middle;
}
@keyframes shimmer { to { background-position: -200% 0; } }
@media (prefers-reduced-motion: reduce) { .skeleton { animation: none; } }
</style>
