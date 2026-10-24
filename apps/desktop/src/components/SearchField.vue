<script setup lang="ts">
// A search box: magnifier, clear button, debounced value, and `/` focuses it while it is on screen.
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import AppIcon from "./AppIcon.vue";

const props = withDefaults(defineProps<{ placeholder: string; debounce?: number }>(), { debounce: 250 });
const model = defineModel<string>({ required: true });
const text = ref(model.value);
const input = ref<HTMLInputElement | null>(null);

let timer: ReturnType<typeof setTimeout> | undefined;
watch(text, (v) => {
  clearTimeout(timer);
  timer = setTimeout(() => (model.value = v), props.debounce);
});
watch(model, (v) => {
  if (v !== text.value) text.value = v;
});

function onKey(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null;
  if (e.key !== "/" || t?.closest("input, textarea, select, [contenteditable]")) return;
  e.preventDefault();
  input.value?.focus();
}
onMounted(() => window.addEventListener("keydown", onKey));
onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKey);
  clearTimeout(timer);
});
function clear() {
  text.value = "";
  model.value = "";
  input.value?.focus();
}
</script>

<template>
  <div class="search">
    <AppIcon name="search" class="icon" />
    <input ref="input" v-model="text" type="search" :placeholder="placeholder" :aria-label="placeholder" @keydown.esc="clear" />
    <button v-if="text" type="button" class="clear" :aria-label="$t('common.clear')" @click="clear"><AppIcon name="close" /></button>
  </div>
</template>

<style scoped>
.search { position: relative; display: flex; align-items: center; min-width: 14rem; flex: 1; max-width: 26rem; }
.icon { position: absolute; left: 0.6rem; color: var(--text-2); pointer-events: none; }
input { padding-left: 2rem !important; padding-right: 2rem !important; }
input::-webkit-search-cancel-button { display: none; }
.clear { position: absolute; right: 0.3rem; background: none; border: 0; color: var(--text-2); cursor: pointer; padding: 0.25rem; display: inline-flex; }
.clear:focus-visible { outline: 2px solid var(--focus); border-radius: var(--radius-xs); }
</style>
