<script setup lang="ts">
import { LOCALES } from "@/i18n";
import { useConnection } from "@/stores/connection";
import { useSettings } from "@/stores/settings";

const settings = useSettings();
const conn = useConnection();
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
        <label class="check">
          <input v-model="settings.notifications" type="checkbox" />
          <span>
            <strong>{{ $t("settings.notifications") }}</strong>
            <span class="small muted block">{{ $t("settings.notificationsHint") }}</span>
          </span>
        </label>
      </section>

      <section class="card placeholder" aria-disabled="true">
        <label class="field">
          <span>{{ $t("settings.bandwidth") }}</span>
          <input disabled :placeholder="$t('settings.bandwidthHint')" />
        </label>
        <label class="field">
          <span>{{ $t("settings.schedule") }}</span>
          <input disabled :placeholder="$t('settings.scheduleHint')" />
        </label>
        <p class="small muted">{{ $t("settings.later") }}</p>
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
.placeholder { opacity: 0.7; display: grid; gap: 0.75rem; }
h2 { font-size: 1rem; margin-top: 0; }
.kv { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 0.35rem 1rem; }
dt { color: var(--text-2); }
dd { margin: 0; word-break: break-all; }
</style>
