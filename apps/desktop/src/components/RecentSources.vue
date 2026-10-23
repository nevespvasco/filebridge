<script setup lang="ts">
// "Recently sent from" (docs/design/specs/D02): one click adds a folder sent before. A source
// that is no longer there (a disk unplugged) is shown, dimmed, and can't be added. Each one can
// be pinned to keep it at hand.
import type { RecentSource } from "@/bindings";
import { baseName, parentPath } from "@/lib/format";
import { useProfiles } from "@/stores/profiles";
import { useSourceShortcuts } from "@/stores/sourceShortcuts";
import AppIcon from "./AppIcon.vue";
import RelativeTime from "./RelativeTime.vue";
import SourceCard from "./SourceCard.vue";

defineProps<{ items: RecentSource[] }>();
const emit = defineEmits<{ add: [path: string] }>();
const profiles = useProfiles();
const shortcuts = useSourceShortcuts();
</script>

<template>
  <section class="recent" :aria-label="$t('source.recent')">
    <h3 class="small muted">{{ $t("source.recent") }}</h3>
    <div class="cards">
      <SourceCard
        v-for="r in items"
        :key="r.path"
        :name="baseName(r.path)"
        :parent="parentPath(r.path)"
        :icon="r.exists ? 'folder' : 'missing'"
        :disabled="!r.exists"
        :tip="r.exists ? $t('source.recentAdd', { path: r.path }) : $t('source.recentMissing', { path: r.path })"
        @add="emit('add', r.path)"
      >
        <template v-if="r.exists">{{ $t("source.recentTo", { destination: profiles.byId[r.last_profile_id]?.name ?? "—" }) }} · <RelativeTime :at="r.last_sent_at" /></template>
        <template v-else>{{ $t("source.recentGone") }}</template>
        <template #side>
          <button
            class="side"
            type="button"
            :aria-pressed="shortcuts.isPinned(r.path)"
            :aria-label="$t(shortcuts.isPinned(r.path) ? 'sourceTools.unpin' : 'sourceTools.pin', { name: baseName(r.path) })"
            :title="shortcuts.isPinned(r.path) ? $t('sourceTools.unpinTip') : $t('sourceTools.pinTip')"
            @click="shortcuts.setPinned(r.path, !shortcuts.isPinned(r.path))"
          ><AppIcon :name="shortcuts.isPinned(r.path) ? 'unpin' : 'pin'" /></button>
        </template>
      </SourceCard>
    </div>
  </section>
</template>

<style scoped>
.recent h3 { margin: 0 0 0.5rem; font-weight: 600; }
.cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(15rem, 100%), 1fr)); gap: 0.6rem; }
</style>
