<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { AppSettings_Deserialize, Autostart } from "@/bindings";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import HelpTip from "@/components/HelpTip.vue";
import SegmentedControl from "@/components/SegmentedControl.vue";
import { LOCALES } from "@/i18n";
import { api, errorMessage } from "@/lib/api";
import { isMobile } from "@/lib/platform";
import { minutesToTime, timeToMinutes } from "@/lib/format";
import { useConnection } from "@/stores/connection";
import { THEMES, useSettings } from "@/stores/settings";

const settings = useSettings();
const conn = useConnection();
const { t } = useI18n();
// Guides (docs/design/specs/D02): on/off, and "show them again" for everyone who skipped them.
const guidesAuto = computed({
  get: () => settings.guides.auto,
  set: (auto: boolean) => (settings.guides = { ...settings.guides, auto }),
});
const guidesReset = ref(false);
function resetGuides() {
  settings.resetGuides();
  guidesReset.value = true;
}
// "Start the engine when I log in" (Linux: the systemd user unit). Read when the screen opens;
// every change shows what it will do and asks first.
const autostart = ref<Autostart | null>(null);
const autostartError = ref<string | null>(null);
const autostartBusy = ref(false);
/** What the open dialog does: turn on, update an outdated unit, or turn off. */
const autostartAsk = ref<"on" | "update" | "off" | null>(null);
const unit = computed(() => (autostart.value?.kind === "unit" ? autostart.value.unit : null));
const autostartOn = computed(() => !!unit.value?.installed && unit.value.enabled);
/** Hidden where it doesn't exist yet (Windows, macOS, phones); shown off with the reason
 *  where it can't work (an AppImage, no systemd). */
const autostartShown = computed(
  () => !isMobile() && autostart.value != null && !(autostart.value.kind === "unsupported" && autostart.value.reason === "os"),
);
const autostartConsequences = computed(() => {
  const u = unit.value;
  if (!u) return [];
  const at = { path: u.path, exec: u.exec };
  if (autostartAsk.value === "off") {
    return [t("settings.autostartOff.remove", at), t("settings.autostartOff.login"), t("settings.autostartOff.now")];
  }
  return [t("settings.autostartOn.write", at), t("settings.autostartOn.login"), t("settings.autostartOn.now"), t("settings.autostartOn.undo")];
});
async function loadAutostart() {
  if (isMobile()) return;
  try {
    autostart.value = await api.autostart();
    autostartError.value = null;
  } catch (e) {
    autostartError.value = errorMessage(e);
  }
}
void loadAutostart();
async function confirmAutostart() {
  const ask = autostartAsk.value;
  if (!ask) return;
  autostartBusy.value = true;
  try {
    autostart.value = await api.setAutostart(ask !== "off");
    autostartError.value = null;
  } catch (e) {
    autostartError.value = errorMessage(e);
  } finally {
    autostartBusy.value = false;
    autostartAsk.value = null;
  }
}

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

      <section v-if="autostartShown" class="card autostart">
        <label class="check">
          <input
            type="checkbox"
            :checked="autostartOn"
            :disabled="!unit || autostartBusy"
            @click.prevent="autostartAsk = autostartOn ? 'off' : 'on'"
          />
          <span>
            <strong>{{ $t("settings.autostart") }}</strong>
            <span class="small muted block">{{ $t("settings.autostartHint") }}</span>
            <span v-if="autostart?.kind === 'unsupported'" class="small muted block">{{ $t(`settings.autostartWhy.${autostart.reason}`) }}</span>
          </span>
        </label>
        <div v-if="autostartOn && unit && !unit.current" class="row">
          <span class="small">{{ $t("settings.autostartOutdated", { exec: unit.exec }) }}</span>
          <button class="btn btn-small" type="button" :disabled="autostartBusy" @click="autostartAsk = 'update'">{{ $t("settings.autostartUpdate") }}</button>
        </div>
        <p v-if="autostartError" class="error small">{{ autostartError }}</p>
      </section>

      <section class="card guides">
        <label class="check">
          <input v-model="guidesAuto" type="checkbox" />
          <span>
            <strong>{{ $t("settings.guides") }}</strong>
            <span class="small muted block">{{ $t("settings.guidesHint") }}</span>
          </span>
        </label>
        <div class="row">
          <button class="btn btn-small" type="button" @click="resetGuides"><AppIcon name="help" /> {{ $t("settings.guidesReset") }}</button>
          <span v-if="guidesReset" class="small ok-text" role="status">{{ $t("settings.guidesResetDone") }}</span>
        </div>
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

    <ConfirmDialog
      :open="autostartAsk != null && unit != null"
      :title="$t(autostartAsk === 'off' ? 'settings.autostartOff.title' : autostartAsk === 'update' ? 'settings.autostartOn.update' : 'settings.autostartOn.title')"
      :consequences="autostartConsequences"
      :confirm-label="$t(autostartAsk === 'off' ? 'settings.autostartOff.confirm' : autostartAsk === 'update' ? 'settings.autostartOn.confirmUpdate' : 'settings.autostartOn.confirm')"
      :busy="autostartBusy"
      @confirm="confirmAutostart"
      @cancel="autostartAsk = null"
    >
      <details v-if="autostartAsk !== 'off' && unit" class="unit-file">
        <summary class="small">{{ $t("settings.autostartFile") }}</summary>
        <pre class="mono small">{{ unit.contents }}</pre>
      </details>
    </ConfirmDialog>
  </div>
</template>

<style scoped>
.list { display: grid; gap: 0.75rem; max-width: 640px; }
.check { display: flex; gap: 0.75rem; align-items: flex-start; cursor: pointer; }
.check input { margin-top: 0.3rem; }
.block { display: block; }
.guides, .autostart { display: grid; gap: 0.75rem; }
.unit-file pre { white-space: pre-wrap; word-break: break-all; margin: 0.5rem 0 0; padding: var(--sp-2); background: var(--surface-2); border-radius: var(--radius-sm); }
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
