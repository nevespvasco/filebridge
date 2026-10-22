<script setup lang="ts">
// "Recently sent from" (docs/design/specs/D02): one click adds a folder sent before. A source
// that is no longer there (a disk unplugged) is shown, dimmed, and can't be added.
import type { RecentSource } from "@/bindings";
import { baseName, parentPath } from "@/lib/format";
import { useProfiles } from "@/stores/profiles";
import AppIcon from "./AppIcon.vue";
import RelativeTime from "./RelativeTime.vue";

defineProps<{ items: RecentSource[] }>();
const emit = defineEmits<{ add: [path: string] }>();
const profiles = useProfiles();
</script>

<template>
  <section class="recent" :aria-label="$t('source.recent')">
    <h3 class="small muted">{{ $t("source.recent") }}</h3>
    <div class="cards">
      <button
        v-for="r in items"
        :key="r.path"
        class="recent-card"
        type="button"
        :aria-disabled="!r.exists"
        :title="r.exists ? $t('source.recentAdd', { path: r.path }) : $t('source.recentMissing', { path: r.path })"
        @click="r.exists && emit('add', r.path)"
      >
        <span class="icon"><AppIcon :name="r.exists ? 'folder' : 'missing'" :size="20" /></span>
        <span class="text">
          <span class="name">{{ baseName(r.path) }}</span>
          <span class="parent small muted">{{ parentPath(r.path) }}</span>
          <span class="meta small muted">
            <template v-if="r.exists">{{ $t("source.recentTo", { destination: profiles.byId[r.last_profile_id]?.name ?? "—" }) }} · <RelativeTime :at="r.last_sent_at" /></template>
            <template v-else>{{ $t("source.recentGone") }}</template>
          </span>
        </span>
        <AppIcon v-if="r.exists" class="plus" name="plus" />
      </button>
    </div>
  </section>
</template>

<style scoped>
.recent h3 { margin: 0 0 0.5rem; font-weight: 600; }
.cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(15rem, 100%), 1fr)); gap: 0.6rem; }
.recent-card {
  font: inherit;
  text-align: left;
  display: flex;
  gap: 0.65rem;
  align-items: center;
  padding: 0.65rem 0.75rem;
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
  background: var(--surface);
  color: var(--text);
  cursor: pointer;
  min-width: 0;
  transition: border-color 0.12s, background 0.12s;
}
.recent-card:hover:not([aria-disabled="true"]) { border-color: var(--accent); background: var(--accent-soft); }
.recent-card:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
.recent-card[aria-disabled="true"] { opacity: 0.55; cursor: not-allowed; border-style: dashed; }
.icon { display: grid; place-items: center; width: 2.2rem; height: 2.2rem; flex: none; border-radius: var(--radius-sm); background: var(--surface-2); color: var(--accent); }
.text { display: grid; min-width: 0; flex: 1; }
.text > span { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.name { font-weight: 600; }
.plus { color: var(--text-2); flex: none; }
@media (prefers-reduced-motion: reduce) { .recent-card { transition: none; } }
</style>
