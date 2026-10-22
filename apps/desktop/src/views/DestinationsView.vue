<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import type { Profile_Serialize } from "@/bindings";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import DestinationSpace from "@/components/DestinationSpace.vue";
import FolderBrowser from "@/components/FolderBrowser.vue";
import DetailsPanel from "@/components/DetailsPanel.vue";
import HelpTip from "@/components/HelpTip.vue";
import PreflightList from "@/components/PreflightList.vue";
import ProfileForm from "@/components/ProfileForm.vue";
import { api, errorMessage } from "@/lib/api";
import { request } from "@/lib/requests";
import { profileKind, profileWhere, useProfiles } from "@/stores/profiles";

const profiles = useProfiles();
const { t } = useI18n();
const creating = ref(false);
/** The destination being browsed (look only). */
const browsing = ref<Profile_Serialize | null>(null);
const deleting = ref<Profile_Serialize | null>(null);
/** The destination whose folder to repair is being picked (docs/04, "Repair destination"). */
const repairing = ref<Profile_Serialize | null>(null);
const router = useRouter();

async function startRepair(dir: string) {
  const p = repairing.value;
  repairing.value = null;
  if (!p) return;
  error.value = null;
  try {
    const id = await api.createRepairJob(p.id, dir || null);
    void router.push({ name: "repair", params: { id } });
  } catch (e) {
    error.value = errorMessage(e);
  }
}
const busy = ref(false);
const error = ref<string | null>(null);

onMounted(async () => {
  await profiles.load();
  void profiles.loadAllStorage();
});

function onSaved(p: Profile_Serialize) {
  creating.value = false;
  void profiles.test(p.id);
}

async function doDelete() {
  if (!deleting.value) return;
  busy.value = true;
  error.value = null;
  try {
    await profiles.remove(deleting.value.id);
    deleting.value = null;
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}

const deleteConsequences = computed(() =>
  deleting.value
    ? [t("destinations.deleteForgets", { name: deleting.value.name }), t("destinations.deleteNoFiles"), t("destinations.deleteJobs")]
    : [],
);

function nextcloudMode(p: Profile_Serialize): string | null {
  const s = p.integration_settings;
  if (s && typeof s === "object" && !Array.isArray(s) && typeof s.mode === "string") return s.mode;
  return null;
}

const details = computed(() => [request("list_profiles"), ...profiles.list.map((p) => request("test_profile", { id: p.id }))]);
</script>

<template>
  <div>
    <header class="page-head">
      <div>
        <h1>{{ $t("destinations.title") }}</h1>
        <p class="lead-text">{{ $t("destinations.intro") }}</p>
      </div>
      <button v-if="!creating && profiles.list.length" class="btn btn-primary" @click="creating = true"><AppIcon name="plus" /> {{ $t("dest.new") }}</button>
    </header>
    <p v-if="profiles.loadError" class="error">{{ profiles.loadError }}</p>
    <p v-if="error" class="error">{{ error }}</p>

    <ProfileForm v-if="creating" class="form" @saved="onSaved" @cancel="creating = false" />

    <div v-if="profiles.loaded && !profiles.list.length && !creating" class="card empty">
      <AppIcon class="empty-icon" name="destinations" :size="40" />
      <strong>{{ $t("destinations.empty") }}</strong>
      <p class="small muted">{{ $t("destinations.emptyHint") }}</p>
      <button class="btn btn-primary" @click="creating = true"><AppIcon name="plus" /> {{ $t("dest.new") }}</button>
    </div>

    <div class="list">
      <section v-for="p in profiles.list" :key="p.id" class="card">
        <div class="head">
          <div class="who">
            <AppIcon :name="profileKind(p)" :size="20" class="kind" />
            <span class="badge badge-muted">{{ $t(`profile.kinds.${profileKind(p)}`) }}</span>
            <strong>{{ p.name }}</strong>
            <span v-if="nextcloudMode(p)" class="small muted">{{ $t("destinations.ncMode", { mode: nextcloudMode(p) }) }}</span>
          </div>
          <div class="actions">
            <button class="btn btn-small" :title="$t('dest.testTip')" :disabled="profiles.tests[p.id]?.running" @click="profiles.test(p.id)">{{ $t("dest.test") }}</button>
            <button class="btn btn-small" :title="$t('destinations.browseTip')" @click="browsing = p"><AppIcon name="reveal" /> {{ $t("destinations.browse") }}</button>
            <button class="btn btn-small" :title="$t('repair.buttonTip')" @click="repairing = p"><AppIcon name="renamed" /> {{ $t("repair.button") }}</button>
            <button class="btn btn-small btn-danger" :title="$t('destinations.deleteTip')" @click="deleting = p">{{ $t("common.delete") }}</button>
          </div>
        </div>
        <p class="mono small muted ellipsis" :title="profileWhere(p)">{{ profileWhere(p) }}</p>
        <DestinationSpace :profile-id="p.id" class="space" />
        <p class="small muted">
          {{ $t(`profile.family.${p.target.family}`) }} · {{ p.target.case_sensitive ? $t("profile.caseSensitive") : $t("profile.caseInsensitive") }}
          <HelpTip :text="$t('profile.help.target')" :label="$t('profile.target')" />
        </p>
        <div v-if="profiles.tests[p.id]" class="test">
          <p v-if="profiles.tests[p.id].running" class="small muted">{{ $t("dest.testing") }}</p>
          <p v-else-if="profiles.tests[p.id].error" class="small error">{{ profiles.tests[p.id].error }}</p>
          <PreflightList
            v-else-if="profiles.tests[p.id].result"
            :views="[profiles.tests[p.id].result!.transport, ...(profiles.tests[p.id].result!.integration ? [profiles.tests[p.id].result!.integration!] : [])]"
          />
        </div>
      </section>
    </div>

    <DetailsPanel :lines="details" />
    <FolderBrowser
      :target="browsing ? { kind: 'profile', id: browsing.id } : null"
      :title="$t('browse.title', { destination: browsing?.name ?? '' })"
      @close="browsing = null"
    />
    <FolderBrowser
      :target="repairing ? { kind: 'profile', id: repairing.id } : null"
      :title="$t('repair.pickTitle', { destination: repairing?.name ?? '' })"
      :choose-label="$t('repair.pick')"
      :allow-new="false"
      @choose="startRepair"
      @close="repairing = null"
    />

    <ConfirmDialog
      :open="!!deleting"
      :title="$t('destinations.deleteTitle')"
      :consequences="deleteConsequences"
      :confirm-label="$t('common.delete')"
      danger
      :busy="busy"
      @confirm="doDelete"
      @cancel="deleting = null"
    />
  </div>
</template>

<style scoped>
.form { margin-bottom: 1rem; }
.page-head .lead-text { margin-bottom: 0; }
.empty { display: grid; justify-items: center; text-align: center; gap: 0.4rem; padding: 2rem 1rem; }
.empty p { margin: 0 0 0.5rem; }
.empty-icon { color: var(--muted); }
.list { display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.75rem; }
.head { display: flex; justify-content: space-between; gap: 1rem; align-items: center; flex-wrap: wrap; }
.kind { align-self: center; color: var(--text-2); }
.who { display: flex; gap: 0.3rem 0.6rem; align-items: baseline; flex-wrap: wrap; min-width: 0; }
.who strong { overflow-wrap: anywhere; }
.actions { display: flex; gap: 0.4rem; flex-wrap: wrap; }
.space { margin: 0.5rem 0; }
.test { margin-top: 0.75rem; border-top: 1px solid var(--border); padding-top: 0.75rem; }
</style>
