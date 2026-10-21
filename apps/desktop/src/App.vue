<script setup lang="ts">
import { computed, onMounted, watch } from "vue";
import { useI18n } from "vue-i18n";
import { commands } from "@/lib/api";
import { isMock } from "@/lib/platform";
import { useConnection } from "@/stores/connection";
import { useJobs, RUNNING } from "@/stores/jobs";
import { useProfiles } from "@/stores/profiles";
import { useSettings } from "@/stores/settings";

const conn = useConnection();
const jobs = useJobs();
const profiles = useProfiles();
const settings = useSettings();
const { t } = useI18n();

onMounted(async () => {
  await jobs.init();
  await conn.init();
  void profiles.load();
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
</script>

<template>
  <div class="shell">
    <nav class="side" :aria-label="$t('nav.label')">
      <div class="brand"><img src="/icon.png" alt="" width="24" height="24" /> filebridge</div>
      <RouterLink to="/new">{{ $t("nav.new") }}</RouterLink>
      <RouterLink to="/jobs">
        {{ $t("nav.jobs") }}
        <span v-if="active" class="badge badge-accent">{{ active }}</span>
      </RouterLink>
      <RouterLink to="/destinations">{{ $t("nav.destinations") }}</RouterLink>
      <RouterLink to="/settings">{{ $t("nav.settings") }}</RouterLink>
      <span class="grow" />
      <div class="conn small" :class="conn.connected ? 'on' : conn.connected === false ? 'off' : 'wait'" :title="conn.endpoint">
        <span class="dot" aria-hidden="true" />
        {{ conn.connected ? $t("conn.connected") : conn.connected === false ? $t("conn.disconnected") : $t("conn.connecting") }}
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
  justify-content: space-between;
  align-items: center;
  padding: 0.5rem 0.6rem;
  border-radius: 6px;
  color: var(--text);
  text-decoration: none;
}
.side a:hover { background: var(--surface-2); }
.side a.router-link-active { background: var(--accent-soft); color: var(--accent); font-weight: 600; }
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
