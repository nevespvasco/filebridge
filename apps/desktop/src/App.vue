<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useFmt } from "@/composables/useFmt";
import { useRouter } from "vue-router";
import { useI18n } from "vue-i18n";
import AppIcon from "@/components/AppIcon.vue";
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
  conn.onReconnect(() => void settings.loadApp());
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
      <div class="brand"><img src="/icon.png" alt="" width="24" height="24" /> filebridge</div>
      <RouterLink to="/new" class="new-transfer" :title="$t('nav.tips.new')">
        <AppIcon name="send" :size="20" /><span class="label">{{ $t("nav.new") }}</span>
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
      <span class="grow" />
      <div v-if="notice" class="notice small" :class="`tone-${notice.tone}`" role="status">
        <AppIcon :name="notice.icon" :class="notice.tone === 'ok' ? 'ok-text' : 'warn-text'" />
        <RouterLink :to="notice.to" class="notice-text">{{ notice.text }}</RouterLink>
        <button class="dismiss" :aria-label="$t('common.dismiss')" @click="dismiss(notice.key, notice.at)"><AppIcon name="close" /></button>
      </div>
      <div class="conn small" :class="conn.connected ? 'on' : conn.connected === false ? 'off' : 'wait'">
        <span class="dot" aria-hidden="true" />
        {{ conn.connected ? $t("conn.connected") : conn.connected === false ? $t("conn.disconnected") : $t("conn.connecting") }}
        <HelpTip :text="$t('conn.tip')" :label="$t('conn.label')" />
      </div>
    </nav>
    <main class="main">
      <div v-if="conn.connected === false" class="banner" role="alert">
        <strong>{{ $t("conn.bannerTitle") }}</strong>
        <span class="small">{{ conn.lastError ?? $t("conn.bannerBody") }}</span>
        <button class="btn btn-small" @click="conn.check()">{{ $t("conn.retry") }}</button>
      </div>
      <RouterView v-slot="{ Component }">
        <KeepAlive include="WizardView">
          <component :is="Component" />
        </KeepAlive>
      </RouterView>
    </main>
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
.dot { width: 8px; height: 8px; border-radius: 50%; background: var(--muted); }
.on .dot { background: var(--ok); }
.off .dot { background: var(--danger); }
.main { padding: 1.5rem 2rem 3rem; max-width: 1100px; width: 100%; box-sizing: border-box; }
.banner {
  display: flex;
  gap: 0.75rem;
  align-items: center;
  flex-wrap: wrap;
  padding: 0.75rem 1rem;
  border-radius: 8px;
  background: var(--danger-soft);
  border: 1px solid var(--danger);
  margin-bottom: 1rem;
}
@media (max-width: 760px) {
  .shell { grid-template-columns: 1fr; }
  .side { flex-direction: row; flex-wrap: wrap; height: auto; position: static; border-right: 0; border-bottom: 1px solid var(--border); }
  .brand { padding: 0.5rem; }
  .grow { display: none; }
  .main { padding: 1rem; }
}
</style>
