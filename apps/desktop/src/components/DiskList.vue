<script setup lang="ts">
// The disks mounted on the computer (docs/design/specs/D02): an external disk or a memory card
// is one click from its folders. The card opens the folder picker on the disk; "Whole disk"
// adds all of it. Not shown on phones.
import type { VolumeView_Serialize } from "@/bindings";
import { useFmt } from "@/composables/useFmt";
import SourceCard from "./SourceCard.vue";

defineProps<{ items: VolumeView_Serialize[] }>();
const emit = defineEmits<{ choose: [path: string]; add: [path: string] }>();
const fmt = useFmt();

/** A drive letter's top folder has no name to send it under (`root` in the source check). */
const isDriveRoot = (path: string) => /^[A-Za-z]:\\?$/.test(path);
</script>

<template>
  <section class="disks" :aria-label="$t('sourceTools.disks')">
    <h3 class="small muted">{{ $t("sourceTools.disks") }}</h3>
    <div class="cards">
      <SourceCard
        v-for="v in items"
        :key="v.path"
        :name="v.label"
        :parent="v.path"
        :icon="v.removable ? 'usb' : 'disk'"
        :tip="$t('sourceTools.diskChoose', { name: v.label })"
        action="reveal"
        @add="emit('choose', v.path)"
      >
        <template v-if="v.removable">{{ $t("sourceTools.diskRemovable") }}</template>
        <template v-if="v.removable && v.total_bytes != null"> · </template>
        <span v-if="v.total_bytes != null" class="num">{{ $t("sourceTools.diskFree", { free: fmt.bytes(v.free_bytes), total: fmt.bytes(v.total_bytes) }) }}</span>
        <template v-if="!isDriveRoot(v.path)" #side>
          <button class="side small" type="button" :title="$t('sourceTools.diskAddAllTip', { name: v.label })" @click="emit('add', v.path)">
            {{ $t("sourceTools.diskAddAll") }}
          </button>
        </template>
      </SourceCard>
    </div>
  </section>
</template>

<style scoped>
.disks h3 { margin: 0 0 0.5rem; font-weight: 600; }
.cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(min(18rem, 100%), 1fr)); gap: 0.6rem; }
</style>
