<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import LoadError from "@/components/LoadError.vue";
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { AppSettings_Deserialize } from "@/bindings";
import HelpTip from "@/components/HelpTip.vue";
import SegmentedControl from "@/components/SegmentedControl.vue";
import { LOCALES } from "@/i18n";
import { errorMessage } from "@/lib/api";
import { minutesToTime, timeToMinutes } from "@/lib/format";
import { modKey, SHORTCUTS } from "@/lib/shortcuts";
import { useConnection } from "@/stores/connection";
import { THEMES, useSettings } from "@/stores/settings";
import { useToast } from "@/stores/toast";

const settings = useSettings();
const conn = useConnection();
const toast = useToast();
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
  toast.show(t("settings.guidesResetDone"));
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
// Every setting is saved as it changes, like the language and the theme: the speed and the hours
// a moment after the last keystroke, and only when valid (start and end can't be the same time).
let timer: ReturnType<typeof setTimeout> | undefined;
watch(draft, () => {
  saved.value = false;
  saveError.value = null;
  clearTimeout(timer);
  if (dirty.value && draft.value.settings) timer = setTimeout(() => void save(), 700);
});
onBeforeUnmount(() => {
  // Leaving the page doesn't lose a change still waiting.
  if (timer && dirty.value && draft.value.settings) void save();
  clearTimeout(timer);
});

async function save() {
  clearTimeout(timer);
  timer = undefined;
  if (!draft.value.settings || !dirty.value) return;
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
    <header class="page-head">
      <div>
        <h1>{{ $t("settings.title") }}</h1>
        <p class="lead-text">{{ $t("settings.intro") }}</p>
      </div>
    </header>
    <div class="groups">
      <section class="group-card card" aria-labelledby="s-general">
        <h2 id="s-general">{{ $t("settings.general") }}</h2>
        <label class="field">
          <span>{{ $t("settings.language") }}</span>
          <select v-model="settings.locale">
            <option v-for="l in LOCALES" :key="l" :value="l">{{ $t(`settings.languages.${l}`) }}</option>
          </select>
        </label>
        <div class="field">
          <span>{{ $t("settings.appearance") }}</span>
          <SegmentedControl v-model="settings.theme" :options="themeOptions" :label="$t('settings.appearance')" />
          <span class="hint">{{ $t("settings.themeHint") }}</span>
        </div>
        <label class="check">
          <input v-model="settings.notifications" type="checkbox" />
          <span>
            <strong>{{ $t("settings.notifications") }}</strong>
            <span class="small muted block">{{ $t("settings.notificationsHint") }}</span>
          </span>
        </label>
        <div class="guides">
          <label class="check">
            <input v-model="guidesAuto" type="checkbox" />
            <span>
              <strong>{{ $t("settings.guides") }}</strong>
              <span class="small muted block">{{ $t("settings.guidesHint") }}</span>
            </span>
          </label>
          <div class="row">
            <button class="btn btn-small" type="button" :disabled="guidesReset" @click="resetGuides"><AppIcon name="help" /> {{ $t("settings.guidesReset") }}</button>
          </div>
        </div>
      </section>

      <section class="group-card card sending" aria-labelledby="s-sending">
        <h2 id="s-sending">{{ $t("settings.sending") }}</h2>
        <LoadError v-if="settings.appError" compact :message="$t('load.sending')" :error="settings.appError" @retry="settings.loadApp()" />

        <div class="group">
          <span class="label-line"><strong>{{ $t("settings.bandwidth") }}</strong><HelpTip :text="$t('settings.bandwidthHelp')" :label="$t('settings.bandwidth')" /></span>
          <label class="check">
            <input v-model="limitOn" type="checkbox" :disabled="!settings.appLoaded || !!settings.appError || conn.offline" />
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
            <input v-model="windowOn" type="checkbox" :disabled="!settings.appLoaded || !!settings.appError || conn.offline" />
            <span>{{ $t("settings.scheduleOnly") }}</span>
          </label>
          <div v-if="windowOn" class="row">
            <label class="inline">{{ $t("settings.from") }} <input v-model="start" type="time" required /></label>
            <label class="inline">{{ $t("settings.to") }} <input v-model="end" type="time" required /></label>
          </div>
          <p v-if="overnight" class="small muted">{{ $t("settings.overnight", { start, end }) }}</p>
        </div>

        <p class="small muted">{{ $t("settings.sendingHint") }}</p>
        <p class="save-state small" role="status">
          <span v-if="draft.error && dirty" class="error">{{ draft.error }}</span>
          <span v-else-if="saveError" class="error">{{ saveError }}</span>
          <span v-else-if="saving" class="muted">{{ $t("settings.saving") }}</span>
          <span v-else-if="saved && !dirty" class="ok-text"><AppIcon name="check" /> {{ $t("settings.saved") }}</span>
        </p>
      </section>

      <section class="group-card card" aria-labelledby="s-keyboard">
        <h2 id="s-keyboard">{{ $t("settings.keyboard") }}</h2>
        <dl class="keys small">
          <template v-for="s in SHORTCUTS" :key="s.key">
            <dt><kbd>{{ modKey }}</kbd> + <kbd>{{ s.key }}</kbd></dt>
            <dd>{{ $t(s.label) }}</dd>
          </template>
        </dl>
      </section>

      <section class="group-card card" aria-labelledby="s-engine">
        <h2 id="s-engine">{{ $t("settings.service") }}</h2>
        <dl class="kv small">
          <dt>{{ $t("settings.state") }}</dt>
          <dd>{{ conn.connected ? $t("conn.connected") : conn.connected === false ? $t("conn.disconnected") : $t("conn.connecting") }}</dd>
          <dt>{{ $t("settings.version") }}</dt>
          <dd>{{ conn.version ?? "—" }}</dd>
          <template v-if="settings.technical">
            <dt>{{ $t("settings.endpoint") }}</dt>
            <dd class="mono">{{ conn.endpoint || "—" }}</dd>
          </template>
        </dl>
        <p class="small muted">{{ $t("settings.serviceHint") }}</p>
        <div><button class="btn btn-small" @click="conn.check()">{{ $t("conn.retry") }}</button></div>
      </section>

      <section class="group-card card" aria-labelledby="s-advanced">
        <h2 id="s-advanced">{{ $t("settings.advanced") }}</h2>
        <label class="check">
          <input v-model="settings.technical" type="checkbox" />
          <span>
            <strong>{{ $t("settings.technical") }}</strong>
            <span class="small muted block">{{ $t("settings.technicalHint") }}</span>
          </span>
        </label>
      </section>
    </div>
  </div>
</template>

<style scoped>
/* A list of two languages doesn't need the card's whole width. */
.group-card > .field > select { max-width: 20rem; }
.groups { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(360px, 100%), 1fr)); gap: var(--sp-3); align-items: start; }
/* min-width 0: a long word (a path) wraps instead of widening the column past the window. */
.group-card { display: grid; gap: var(--sp-4); min-width: 0; }
.group-card h2 { font-size: var(--fs-base); margin: 0; }
.check { display: flex; gap: var(--sp-3); align-items: flex-start; cursor: pointer; }
.check input { margin-top: 0.3rem; }
.block { display: block; }
.guides { display: grid; gap: var(--sp-3); }
.group { display: grid; gap: var(--sp-2); }
.group > .label-line { display: inline-flex; align-items: center; }
.row { display: flex; gap: var(--sp-2) var(--sp-3); align-items: center; flex-wrap: wrap; }
.sending .row input, .sending .row select { width: auto; }
.sending .row .num { width: 8rem; }
.sending p { margin: 0; }
.save-state { min-height: 1.3em; }
.inline { display: inline-flex; gap: var(--sp-2); align-items: center; }
.kv, .keys { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: var(--sp-2) var(--sp-4); margin: 0; }
dt { color: var(--text-2); }
dd { margin: 0; word-break: break-word; }
kbd { font-family: var(--font-mono); font-size: var(--fs-xs); padding: 0.05rem 0.4rem; border: 1px solid var(--border-strong); border-bottom-width: 2px; border-radius: var(--radius-sm); background: var(--surface-2); color: var(--text); }
</style>
