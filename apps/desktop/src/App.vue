<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useFmt } from "@/composables/useFmt";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import AppIcon from "@/components/AppIcon.vue";
import GuidedTour from "@/components/GuidedTour.vue";
import HelpTip from "@/components/HelpTip.vue";
import { commands } from "@/lib/api";
import { isMock } from "@/lib/platform";
import { useConnection } from "@/stores/connection";
import { needsYou, useJobs, RUNNING } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";
import { useReclaim } from "@/stores/reclaim";
import { useSettings } from "@/stores/settings";

const conn = useConnection();
const jobs = useJobs();
const profiles = useProfiles();
const settings = useSettings();
const reclaim = useReclaim();
const { t } = useI18n();
const router = useRouter();
const fmt = useFmt();

onMounted(async () => {
  await jobs.init();
  await conn.init();
  void profiles.load().then(() => profiles.loadAllStorage());
  void reclaim.loadEligible();
  void settings.loadApp();
  conn.onReconnect(() => {
    void settings.loadApp();
    void profiles.load().then(() => profiles.loadAllStorage());
    void reclaim.loadEligible();
  });
  void jobs.loadPage(0);
});

// The tray menu is built in Rust; its labels come from here so they follow the language.
watch(
  () => settings.locale,
  () => {
    if (!isMock) void commands.setTrayLabels(t("tray.show"), t("tray.quit"));
  },
  { immediate: true },
);

/** What the banner says: an old or new engine, one that can't start, or one that stopped. */
const problem = computed(() => {
  if (conn.lastCode === "incompatible_service") return "version";
  if (conn.lastCode === "service_unavailable") return "start";
  return "lost";
});

const active = computed(() => Object.values(jobs.byId).filter((j) => RUNNING.includes(j.state)).length);
// The sidebar's notice slot (docs/design/02, "Sidebar"): one notice at most. A dismissed one stays
// hidden until its condition is new (here: a newer measurement of the same destination).
const dismissed = ref<Record<string, number>>(readDismissed());
function readDismissed(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem("filebridge.dismissed") ?? "{}") as Record<string, number>;
  } catch {
    return {};
  }
}
function dismiss(key: string, at: number) {
  dismissed.value = { ...dismissed.value, [key]: at };
  try {
    localStorage.setItem("filebridge.dismissed", JSON.stringify(dismissed.value));
  } catch {
    // The notice comes back next time.
  }
}
const notice = computed(() => {
  for (const p of profiles.low) {
    const s = profiles.storage[p.id];
    const key = `low:${p.id}`;
    if (s?.checked_at != null && dismissed.value[key] !== s.checked_at) {
      return { key, at: s.checked_at, text: t("storage.lowNotice", { name: p.name, free: fmt.bytes(s.free_bytes) }), to: "/destinations", icon: "warning" as const, tone: "warn" };
    }
  }
  // Lowest priority: space that can be freed at the source (D10), hidden per set of jobs.
  if (reclaim.eligible.length) {
    const key = `reclaim:${reclaim.eligible.map((s) => s.job_id).join(",")}`;
    const at = Math.max(...reclaim.eligible.map((s) => s.checked_at ?? 0));
    if (dismissed.value[key] !== at) return { key, at, text: t("reclaim.notice", { size: fmt.bytes(reclaim.totalBytes) }), to: `/jobs/${reclaim.eligible[0].job_id}`, icon: "broom" as const, tone: "ok" };
  }
  return null;
});

/** Jobs waiting for the user (decisions, a failure, a long retry): the dot on Transfers. */
const attention = computed(() => Object.values(jobs.byId).filter((j) => needsYou(j)).length);

// Ctrl/⌘+N: new transfer; Ctrl/⌘+1…5: the sidebar's places (docs/design/02, "Keyboard").
const PLACES = ["/", "/new", "/jobs", "/destinations", "/settings"];
function onKey(e: KeyboardEvent) {
  if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
  const target = e.target as HTMLElement | null;
  if (target?.closest("input, textarea, select, [contenteditable]")) return;
  const key = e.key.toLowerCase();
  const to = key === "n" ? "/new" : PLACES[Number(key) - 1];
  if (!to) return;
  e.preventDefault();
  void router.push(to);
}
onMounted(() => window.addEventListener("keydown", onKey));
onBeforeUnmount(() => window.removeEventListener("keydown", onKey));
</script>

<template>
  <div class="shell">
    <nav class="side" :aria-label="$t('nav.label')">
      <div class="brand"><img src="/icon.png" alt="" width="24" height="24" /> <span>filebridge</span></div>
      <RouterLink to="/new" class="new-transfer" :title="$t('nav.tips.new')">
        <AppIcon name="send" :size="20" /><span class="label">{{ $t("nav.new") }}</span>
      </RouterLink>
      <div class="places">
        <RouterLink to="/home" :title="$t('nav.tips.home')">
          <AppIcon name="home" :size="20" /><span class="label">{{ $t("nav.home") }}</span>
        </RouterLink>
        <RouterLink to="/jobs" :title="$t('nav.tips.jobs')">
          <AppIcon name="transfers" :size="20" /><span class="label">{{ $t("nav.jobs") }}</span>
          <span v-if="attention" class="attention" role="img" :aria-label="$t('nav.attention', { n: attention }, attention)" :title="$t('nav.attention', { n: attention }, attention)" />
          <span v-if="active" class="badge badge-accent num" :aria-label="$t('nav.active', { n: active }, active)">{{ active }}</span>
        </RouterLink>
        <RouterLink to="/destinations" :title="$t('nav.tips.destinations')">
          <AppIcon name="destinations" :size="20" /><span class="label">{{ $t("nav.destinations") }}</span>
        </RouterLink>
        <RouterLink to="/settings" :title="$t('nav.tips.settings')">
          <AppIcon name="settings" :size="20" /><span class="label">{{ $t("nav.settings") }}</span>
        </RouterLink>
      </div>
      <span class="grow" />
      <div v-if="notice" class="notice small" :class="`tone-${notice.tone}`" role="status">
        <AppIcon :name="notice.icon" :class="notice.tone === 'ok' ? 'ok-text' : 'warn-text'" />
        <RouterLink :to="notice.to" class="notice-text">{{ notice.text }}</RouterLink>
        <button class="dismiss" :aria-label="$t('common.dismiss')" @click="dismiss(notice.key, notice.at)"><AppIcon name="close" /></button>
      </div>
      <div class="conn small" :class="conn.connected ? 'on' : conn.connected === false ? 'off' : 'wait'">
        <span class="dot" aria-hidden="true" :title="conn.connected ? $t('conn.connected') : undefined" />
        <span class="conn-text">{{ conn.connected ? $t("conn.connected") : conn.connected === false ? $t("conn.disconnected") : $t("conn.connecting") }}</span>
        <!-- The top bar's short form; the banner under it says what happened and what to do. -->
        <span v-if="!conn.connected" class="conn-short" aria-hidden="true">{{ conn.connected === false ? $t("conn.short.disconnected") : $t("conn.short.connecting") }}</span>
        <HelpTip :text="$t('conn.tip')" :label="$t('conn.label')" />
      </div>
    </nav>
    <main class="main">
      <div v-if="conn.connected === false" class="banner" role="alert">
        <AppIcon name="warning" :size="20" class="banner-icon" />
        <div class="banner-text">
          <strong>{{ $t(`conn.problem.${problem}.title`) }}</strong>
          <span class="small">{{ $t(`conn.problem.${problem}.body`) }}</span>
          <details v-if="conn.lastError" class="small muted">
            <summary>{{ $t("conn.details") }}</summary>
            <code>{{ conn.lastError }}</code>
          </details>
        </div>
        <button class="btn btn-small" :disabled="conn.checking" @click="conn.check()">
          <AppIcon name="retry" /> {{ conn.checking ? $t("conn.connecting") : $t("conn.retry") }}
        </button>
      </div>
      <RouterView v-slot="{ Component }">
        <KeepAlive include="WizardView">
          <component :is="Component" />
        </KeepAlive>
      </RouterView>
    </main>
    <GuidedTour />
  </div>
</template>

<style scoped>
.shell { display: grid; grid-template-columns: 200px minmax(0, 1fr); min-height: 100vh; }
.side {
  display: flex;
  flex-direction: column;
  gap: 0.15rem;
  padding: 1rem 0.6rem;
  background: var(--surface);
  border-right: 1px solid var(--border);
  position: sticky;
  top: 0;
  height: 100vh;
  /* A short window (the minimum is 480px tall) scrolls the sidebar, never cuts it off. */
  overflow-y: auto;
}
.brand { display: flex; gap: 0.5rem; align-items: center; font-weight: 700; font-size: 1.1rem; padding: 0 0.5rem 1rem; }
.side a {
  display: flex;
  gap: 0.6rem;
  align-items: center;
  padding: 0.5rem 0.6rem;
  border-radius: 6px;
  color: var(--text);
  text-decoration: none;
}
.side a:hover { background: var(--surface-2); }
.side a .label { flex: 1; }
.side a.router-link-active { background: var(--accent-soft); color: var(--accent); font-weight: 600; }
.side a:focus-visible { outline: 2px solid var(--focus); outline-offset: 1px; }
/* The one primary action of the sidebar (docs/design/02, "Sidebar"). */
.side a.new-transfer { background: var(--accent); color: var(--on-accent); font-weight: 600; margin-bottom: 0.6rem; }
.side a.new-transfer:hover { filter: brightness(1.08); background: var(--accent); }
.side a.new-transfer.router-link-active { box-shadow: 0 0 0 2px var(--accent-soft), 0 0 0 4px var(--accent); }
.attention { width: 7px; height: 7px; border-radius: 50%; background: var(--warn); flex: none; }
.notice { display: flex; gap: 0.4rem; align-items: flex-start; padding: 0.55rem 0.6rem; border-radius: var(--radius-sm); background: var(--warn-soft); border: 1px solid color-mix(in srgb, var(--warn) 40%, transparent); margin: 0 0.1rem 0.5rem; }
.notice.tone-ok { background: var(--ok-soft); border-color: color-mix(in srgb, var(--ok) 40%, transparent); }
.side a.notice-text { all: unset; flex: 1; cursor: pointer; color: var(--text); }
.side a.notice-text:hover { text-decoration: underline; }
.dismiss { background: none; border: 0; padding: 0; color: var(--text-2); cursor: pointer; display: inline-flex; }
.conn { display: flex; gap: 0.4rem; align-items: center; padding: 0.5rem; color: var(--text-2); }
.conn-short { display: none; }
.dot { width: 8px; height: 8px; border-radius: 50%; background: var(--muted); }
.on .dot { background: var(--ok); }
.off .dot { background: var(--danger); }
/* The main column: capped and centred, so a wide monitor gets even margins, not one empty side. */
.main { padding: 1.5rem 2rem 3rem; max-width: 1100px; width: 100%; margin-inline: auto; box-sizing: border-box; min-width: 0; }
.banner {
  display: flex;
  gap: 0.75rem;
  align-items: flex-start;
  padding: 0.75rem 1rem;
  border-radius: 8px;
  background: var(--danger-soft);
  border: 1px solid var(--danger);
  margin-bottom: 1rem;
}
.banner-icon { color: var(--danger); flex: none; margin-top: 0.1rem; }
.banner-text { display: grid; gap: 0.2rem; flex: 1; min-width: 0; }
.banner-text details { margin-top: 0.2rem; }
.banner-text summary { cursor: pointer; }
.banner-text code { display: block; margin-top: 0.25rem; overflow-wrap: anywhere; }
.places { display: contents; }
@media (min-width: 1600px) {
  .main { max-width: 1280px; padding-inline: 2.5rem; }
}
@media (max-width: 1099px) {
  .main { padding: 1.25rem 1.5rem 2.5rem; }
}
/* Below 760px the sidebar splits in two (docs/design/02, "Window sizes"), both always on screen:
   a top bar (brand, status, New transfer, and the notice as a strip) and a tab bar at the bottom
   with the four places. The bar's height is `--nav-bottom` (style.css), which whatever sticks to
   the bottom of the page stands on. */
@media (max-width: 759px) {
  .shell { grid-template-columns: minmax(0, 1fr); grid-template-rows: auto 1fr; }
  .side {
    display: grid;
    align-content: start;
    grid-template-columns: minmax(0, 1fr) auto auto;
    grid-template-areas: "brand conn new" "notice notice notice";
    align-items: center;
    gap: 0.5rem 0.4rem;
    height: auto;
    padding: 0.5rem 0.75rem;
    position: sticky;
    top: 0;
    z-index: 20;
    overflow: visible;
    border-right: 0;
    border-bottom: 1px solid var(--border);
  }
  .brand { grid-area: brand; padding: 0.25rem; min-width: 0; overflow: hidden; }
  .brand span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .side a.new-transfer { grid-area: new; margin: 0; padding: 0.4rem 0.75rem; }
  .places {
    position: fixed;
    inset: auto 0 0 0;
    z-index: 20;
    display: flex;
    gap: 0.15rem;
    height: calc(var(--nav-bottom) + env(safe-area-inset-bottom, 0px));
    padding: 0 0.5rem env(safe-area-inset-bottom, 0px);
    background: var(--surface);
    border-top: 1px solid var(--border);
    box-shadow: 0 -4px 12px -6px rgb(0 0 0 / 0.15);
  }
  .places a { flex: 1 1 0; min-width: 0; flex-direction: column; justify-content: center; gap: 0.15rem; font-size: var(--fs-xs); padding: 0.3rem 0.2rem; border-radius: 0; border-top: 2px solid transparent; position: relative; }
  .places a .label { flex: none; white-space: nowrap; }
  .side .places a.router-link-active { background: transparent; border-top-color: var(--accent); }
  .places a .badge { position: absolute; top: 0.25rem; left: calc(50% + 0.5rem); padding: 0 0.35rem; }
  .places a .attention { position: absolute; top: 0.45rem; left: calc(50% - 0.9rem); }
  .grow { display: none; }
  .notice { grid-area: notice; margin: 0; }
  .conn { grid-area: conn; padding: 0.25rem; gap: 0.25rem; }
  /* The full sentence is for screen readers only. Connected is the usual state, and the green dot
     alone says it (with its tooltip); connecting or offline is a short badge next to it. */
  .conn-text { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
  .conn:not(.on) { gap: 0.35rem; }
  .conn:not(.on) .dot { display: none; }
  .conn-short { display: inline-flex; align-items: center; gap: 0.35rem; font-size: var(--fs-xs); font-weight: 600; white-space: nowrap; padding: 0.15rem 0.55rem; border-radius: 999px; background: var(--surface-2); color: var(--text-2); }
  .conn-short::before { content: ""; width: 7px; height: 7px; border-radius: 50%; background: var(--muted); }
  .off .conn-short { background: var(--danger-soft); color: var(--danger); }
  .off .conn-short::before { background: var(--danger); }
  /* The banner's button goes under its text when they don't fit side by side. */
  .banner { flex-wrap: wrap; }
  .banner-text { flex: 1 1 12rem; }
  .banner > .btn { margin-left: calc(20px + 0.75rem); }
  .main { padding: 1rem 1rem calc(var(--nav-bottom) + 2rem); }
}
/* A phone-sized window: New transfer keeps its icon and its accessible name, not its text. */
@media (max-width: 399px) {
  .side a.new-transfer .label { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
  .side a.new-transfer { padding: 0.45rem 0.6rem; }
  .main { padding-inline: 0.75rem; }
}
</style>
