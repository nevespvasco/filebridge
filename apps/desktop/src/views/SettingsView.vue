<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { AppSettings_Deserialize } from "@/bindings";
import HelpTip from "@/components/HelpTip.vue";
import SegmentedControl from "@/components/SegmentedControl.vue";
import { LOCALES } from "@/i18n";
import { errorMessage } from "@/lib/api";
import { minutesToTime, timeToMinutes } from "@/lib/format";
import { useConnection } from "@/stores/connection";
import { THEMES, useSettings } from "@/stores/settings";

const settings = useSettings();
const conn = useConnection();
const { t } = useI18n();
const themeOptions = computed(() => THEMES.map((value) => ({ value, label: t(`settings.themes.${value}`), icon: value })));

/** Bytes per second in one of each unit (decimal, like the rest of the app). */
const UNIT_BYTES = { mbit: 125_000, mbyte: 1_000_000 } as const;
type Unit = keyof typeof UNIT_BYTES;

// The form, filled from the service's settings whenever they are (re)read.
const limitOn = ref(false);
const limitValue = ref<number | null>(null);
const limitUnit = ref<Unit>("mbit");
const windowOn = ref(false);
const start = ref("22:00");
const end = ref("07:00");
const saving = ref(false);
const saved = ref(false);
const saveError = ref<string | null>(null);

function fill() {
  const a = settings.app;
  limitOn.value = a.bandwidth_limit != null;
  if (a.bandwidth_limit != null) {
    limitUnit.value = a.bandwidth_limit % UNIT_BYTES.mbyte === 0 ? "mbyte" : "mbit";
    limitValue.value = Math.round((a.bandwidth_limit / UNIT_BYTES[limitUnit.value]) * 100) / 100;
  }
  windowOn.value = a.send_window != null;
  if (a.send_window) {
    start.value = minutesToTime(a.send_window.start_minute);
    end.value = minutesToTime(a.send_window.end_minute);
  }
}
watch(() => settings.app, fill, { immediate: true });

/** What the form would save, or why it can't. */
const draft = computed<{ settings?: AppSettings_Deserialize; error?: string }>(() => {
  let bandwidth_limit: number | null = null;
  if (limitOn.value) {
    const v = Number(limitValue.value);
    if (!Number.isFinite(v) || v <= 0) return { error: t("settings.badLimit") };
    bandwidth_limit = Math.max(1024, Math.round(v * UNIT_BYTES[limitUnit.value]));
  }
  let send_window: AppSettings_Deserialize["send_window"] = null;
  if (windowOn.value) {
    const [s, e] = [timeToMinutes(start.value), timeToMinutes(end.value)];
    if (s == null || e == null || s === e) return { error: t("settings.badWindow") };
    send_window = { start_minute: s, end_minute: e };
  }
  return { settings: { bandwidth_limit, send_window } };
});
const dirty = computed(() => JSON.stringify(draft.value.settings) !== JSON.stringify(settings.app));
const overnight = computed(() => {
  const w = draft.value.settings?.send_window;
  return !!w && w.start_minute > w.end_minute;
});
watch(draft, () => {
  saved.value = false;
  saveError.value = null;
});

async function save() {
  if (!draft.value.settings) return;
  saving.value = true;
  try {
    await settings.saveApp(draft.value.settings);
    saved.value = true;
  } catch (e) {
    saveError.value = errorMessage(e);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div>
    <header class="page-head"><h1>{{ $t("settings.title") }}</h1></header>
    <div class="list">
      <section class="card">
        <label class="field">
          <span>{{ $t("settings.language") }}</span>
          <select v-model="settings.locale">
            <option v-for="l in LOCALES" :key="l" :value="l">{{ $t(`settings.languages.${l}`) }}</option>
          </select>
        </label>
      </section>

      <section class="card">
        <div class="field">
          <span>{{ $t("settings.appearance") }}</span>
          <SegmentedControl v-model="settings.theme" :options="themeOptions" :label="$t('settings.appearance')" />
          <span class="hint">{{ $t("settings.themeHint") }}</span>
        </div>
      </section>

      <section class="card">
        <label class="check">
          <input v-model="settings.notifications" type="checkbox" />
          <span>
            <strong>{{ $t("settings.notifications") }}</strong>
            <span class="small muted block">{{ $t("settings.notificationsHint") }}</span>
          </span>
        </label>
      </section>

      <section class="card sending">
        <h2>{{ $t("settings.sending") }}</h2>
        <p v-if="settings.appError" class="error small">{{ settings.appError }}</p>

        <div class="group">
          <span class="label-line"><strong>{{ $t("settings.bandwidth") }}</strong><HelpTip :text="$t('settings.bandwidthHelp')" :label="$t('settings.bandwidth')" /></span>
          <label class="check">
            <input v-model="limitOn" type="checkbox" :disabled="!settings.appLoaded" />
            <span>{{ $t("settings.bandwidthLimit") }}</span>
          </label>
          <div v-if="limitOn" class="row">
            <input v-model.number="limitValue" class="num" type="number" min="0" step="any" inputmode="decimal" :aria-label="$t('settings.bandwidth')" />
            <select v-model="limitUnit" :aria-label="$t('settings.bandwidthUnit')">
              <option v-for="(_, u) in UNIT_BYTES" :key="u" :value="u">{{ $t(`settings.units.${u}`) }}</option>
            </select>
          </div>
        </div>

        <div class="group">
          <span class="label-line"><strong>{{ $t("settings.schedule") }}</strong><HelpTip :text="$t('settings.scheduleHelp')" :label="$t('settings.schedule')" /></span>
          <label class="check">
            <input v-model="windowOn" type="checkbox" :disabled="!settings.appLoaded" />
            <span>{{ $t("settings.scheduleOnly") }}</span>
          </label>
          <div v-if="windowOn" class="row">
            <label class="inline">{{ $t("settings.from") }} <input v-model="start" type="time" required /></label>
            <label class="inline">{{ $t("settings.to") }} <input v-model="end" type="time" required /></label>
          </div>
          <p v-if="overnight" class="small muted">{{ $t("settings.overnight", { start, end }) }}</p>
        </div>

        <p class="small muted">{{ $t("settings.sendingHint") }}</p>
        <div class="row">
          <button class="btn btn-primary btn-small" :disabled="!settings.appLoaded || !dirty || !!draft.error || saving" @click="save()">
            {{ $t("settings.save") }}
          </button>
          <span v-if="draft.error && dirty" class="error small">{{ draft.error }}</span>
          <span v-else-if="saveError" class="error small">{{ saveError }}</span>
          <span v-else-if="saved && !dirty" class="ok-text small"><AppIcon name="check" /> {{ $t("settings.saved") }}</span>
        </div>
      </section>

      <section class="card">
        <h2>{{ $t("settings.service") }}</h2>
        <dl class="kv small">
          <dt>{{ $t("settings.state") }}</dt>
          <dd>{{ conn.connected ? $t("conn.connected") : conn.connected === false ? $t("conn.disconnected") : $t("conn.connecting") }}</dd>
          <dt>{{ $t("settings.version") }}</dt>
          <dd>{{ conn.version ?? "—" }}</dd>
          <dt>{{ $t("settings.endpoint") }}</dt>
          <dd class="mono">{{ conn.endpoint || "—" }}</dd>
        </dl>
        <p class="small muted">{{ $t("settings.serviceHint") }}</p>
        <button class="btn btn-small" @click="conn.check()">{{ $t("conn.retry") }}</button>
      </section>
    </div>
  </div>
</template>

<style scoped>
.list { display: grid; gap: 0.75rem; max-width: 640px; }
.check { display: flex; gap: 0.75rem; align-items: flex-start; cursor: pointer; }
.check input { margin-top: 0.3rem; }
.block { display: block; }
.sending { display: grid; gap: 0.9rem; }
.sending h2 { margin-bottom: 0; }
.group { display: grid; gap: 0.4rem; }
.group > .label-line { display: inline-flex; align-items: center; }
.row { display: flex; gap: 0.5rem 0.75rem; align-items: center; flex-wrap: wrap; }
.sending .row input, .sending .row select { width: auto; }
.sending .row .num { width: 8rem; }
.inline { display: inline-flex; gap: 0.4rem; align-items: center; }
h2 { font-size: 1rem; margin-top: 0; }
.kv { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 0.35rem 1rem; }
dt { color: var(--text-2); }
dd { margin: 0; word-break: break-all; }
</style>
