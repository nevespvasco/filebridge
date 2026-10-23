<script setup lang="ts">
// Pinned sources (docs/design/specs/D02): folders and files kept at hand, added in one click. A
// pinned one that isn't there now (a disk unplugged) is dimmed and can't be added; unpinning
// only takes it off this list.
import type { PinnedSource } from "@/bindings";
import { baseName, parentPath } from "@/lib/format";
import { useSourceShortcuts } from "@/stores/sourceShortcuts";
import AppIcon from "./AppIcon.vue";
import SourceCard from "./SourceCard.vue";

defineProps<{ items: PinnedSource[] }>();
const emit = defineEmits<{ add: [path: string] }>();
const shortcuts = useSourceShortcuts();
</script>

<template>
  <section class="pinned" :aria-label="$t('sourceTools.pinned')">
    <h3 class="small muted">{{ $t("sourceTools.pinned") }}</h3>
    <div class="cards">
      <SourceCard
        v-for="p in items"
        :key="p.path"
        :name="baseName(p.path) || p.path"
        :parent="parentPath(p.path)"
        :icon="p.exists ? 'pin' : 'missing'"
        :disabled="!p.exists"
        :tip="p.exists ? $t('sourceTools.pinnedAdd', { path: p.path }) : $t('sourceTools.pinnedMissing', { path: p.path })"
        @add="emit('add', p.path)"
      >
        <template v-if="!p.exists">{{ $t("sourceTools.pinnedGone") }}</template>
        <template #side>
          <button
            class="side"
            type="button"
            :aria-label="$t('sourceTools.unpin', { name: baseName(p.path) || p.path })"
            :title="$t('sourceTools.unpinTip')"
            @click="shortcuts.setPinned(p.path, false)"
          ><AppIcon name="unpin" /></button>
        </template>
      </SourceCard>
    </div>
    <p v-if="shortcuts.pinError" class="small error" role="alert">{{ $t("sourceTools.pinFailed", { error: shortcuts.pinError }) }}</p>
  </section>
</template>

<style scoped>
.pinned h3 { margin: 0 0 0.5rem; font-weight: 600; }
.pinned p { margin: 0.4rem 0 0; }
.cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(15rem, 100%), 1fr)); gap: 0.6rem; }
</style>
