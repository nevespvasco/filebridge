<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { computed, onMounted, ref, shallowRef } from "vue";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import type { Profile_Serialize } from "@/bindings";
import AccessDialog from "@/components/AccessDialog.vue";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import DestinationSpace from "@/components/DestinationSpace.vue";
import FolderBrowser from "@/components/FolderBrowser.vue";
import DetailsPanel from "@/components/DetailsPanel.vue";
import HelpTip from "@/components/HelpTip.vue";
import HostKeyPrompt from "@/components/HostKeyPrompt.vue";
import PreflightList from "@/components/PreflightList.vue";
import ProfileForm from "@/components/ProfileForm.vue";
import { api, errorMessage } from "@/lib/api";
import { request } from "@/lib/requests";
import { profileKind, profileWhere, sshHosts, sshKeyAction, useProfiles, type TestState } from "@/stores/profiles";

const profiles = useProfiles();
const { t } = useI18n();
const creating = ref(false);
/** The destination being browsed (look only). */
const browsing = ref<Profile_Serialize | null>(null);
const deleting = shallowRef<Profile_Serialize | null>(null);
/** filebridge's own SSH key (ADR 0012): the destination it is installed on, or removed from. */
const access = shallowRef<{ profile: Profile_Serialize; action: "install" | "remove" } | null>(null);
/** SSH destinations can get filebridge's own key; those that have it can have it removed. */
const sshKey = sshKeyAction;

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
  void test(p);
}

// SSH servers whose host key isn't in known_hosts (it was cleared, or this is a new device) make
// every connection fail. After a failed test or space check, and before browsing, each server
// the destination signs in to is checked: an unknown key is shown to the user to trust (ADR
// 0015), and what failed is tried again. A changed key only shows the error.
const hostKey = ref<InstanceType<typeof HostKeyPrompt> | null>(null);
/** Destinations whose servers' keys are being checked. */
const checkingKeys = ref<Record<string, boolean>>({});
/** One check at a time: a second prompt would cancel the first. */
let keyQueue: Promise<unknown> = Promise.resolve();

/** Checks the key of each SSH server of `p`: `trusted` when one was trusted just now, `refused`
 *  when the user didn't trust one or its key changed, otherwise `known`. A server that can't be
 *  reached also gives `known`: what failed (test, check, browser) already says why. */
function checkKeys(p: Profile_Serialize): Promise<"known" | "trusted" | "refused"> {
  const run = async () => {
    let out: "known" | "trusted" | "refused" = "known";
    checkingKeys.value[p.id] = true;
    try {
      for (const h of sshHosts(p)) {
        const r = await hostKey.value!.check(h.host, h.port, h.knownHostsFile);
        if (r === "refused") return "refused";
        if (r === "trusted") out = r;
      }
    } catch {
      // Unreachable.
    } finally {
      checkingKeys.value[p.id] = false;
    }
    return out;
  };
  const next = keyQueue.then(run, run);
  keyQueue = next;
  return next;
}

function failed(s: TestState): boolean {
  const r = s.result;
  return !!s.error || (!!r && [r.transport, r.integration].some((v) => v?.checks.some((c) => c.status === "fail")));
}

async function test(p: Profile_Serialize) {
  const s = await profiles.test(p.id);
  if (sshHosts(p).length && failed(s) && (await checkKeys(p)) === "trusted") await profiles.test(p.id);
}

async function checkSpace(p: Profile_Serialize) {
  const s = await profiles.loadStorage(p.id, true);
  if (sshHosts(p).length && s?.last_error && (await checkKeys(p)) === "trusted") await profiles.loadStorage(p.id, true);
}

/** Browsing needs the servers' keys first, as in the wizard. */
async function browse(p: Profile_Serialize, open: (p: Profile_Serialize) => void) {
  if (!sshHosts(p).length || (await checkKeys(p)) !== "refused") open(p);
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
/** Deleting forgets filebridge's own key here, but its line stays on the server (ADR 0012). */
const deletingWithKey = computed(() => !!deleting.value && sshKey(deleting.value) === "remove");
function removeAccessFirst() {
  const p = deleting.value;
  deleting.value = null;
  if (p) access.value = { profile: p, action: "remove" };
}

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
            <span v-if="nextcloudMode(p)" class="small muted">{{ $t("destinations.ncMode", { mode: $t(`webdav.modeShort.${nextcloudMode(p)}`) }) }}</span>
          </div>
          <div class="actions">
            <button class="btn btn-small" :title="$t('dest.testTip')" :disabled="profiles.tests[p.id]?.running || checkingKeys[p.id]" @click="test(p)">{{ $t("dest.test") }}</button>
            <button class="btn btn-small" :title="$t('destinations.browseTip')" :disabled="checkingKeys[p.id]" @click="browse(p, (x) => (browsing = x))"><AppIcon name="reveal" /> {{ $t("destinations.browse") }}</button>
            <button class="btn btn-small" :title="$t('repair.buttonTip')" :disabled="checkingKeys[p.id]" @click="browse(p, (x) => (repairing = x))"><AppIcon name="renamed" /> {{ $t("repair.button") }}</button>
            <button
              v-if="sshKey(p)"
              class="btn btn-small"
              :title="$t(`access.${sshKey(p)}.tip`)"
              @click="access = { profile: p, action: sshKey(p)! }"
            >
              <AppIcon :name="sshKey(p) === 'install' ? 'key' : 'lock'" /> {{ $t(`access.${sshKey(p)}.button`) }}
            </button>
            <button class="btn btn-small btn-danger" :title="$t('destinations.deleteTip')" @click="deleting = p">{{ $t("common.delete") }}</button>
          </div>
        </div>
        <p class="mono small muted ellipsis" :title="profileWhere(p)">{{ profileWhere(p) }}</p>
        <p v-if="sshKey(p) === 'remove'" class="small ok-text"><AppIcon name="key" :size="16" /> {{ $t("access.uses") }}</p>
        <DestinationSpace :profile-id="p.id" class="space" :refresh="() => checkSpace(p)" />
        <p v-if="checkingKeys[p.id]" class="small muted">{{ $t("hostKey.checking") }}</p>
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

    <HostKeyPrompt ref="hostKey" />
    <AccessDialog :profile="access?.profile ?? null" :action="access?.action ?? 'install'" @close="access = null" />

    <ConfirmDialog
      :open="!!deleting"
      :title="$t('destinations.deleteTitle')"
      :consequences="deleteConsequences"
      :confirm-label="$t('common.delete')"
      danger
      :busy="busy"
      @confirm="doDelete"
      @cancel="deleting = null"
    >
      <div v-if="deletingWithKey" class="key-warning" role="note">
        <p class="small error"><AppIcon name="key" :size="16" /> {{ $t("destinations.deleteKey") }}</p>
        <button class="btn btn-small" :disabled="busy" @click="removeAccessFirst"><AppIcon name="lock" /> {{ $t("destinations.deleteRemoveFirst") }}</button>
      </div>
    </ConfirmDialog>
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
.key-warning { margin: 0.5rem 0; }
.key-warning p { margin: 0 0 0.4rem; }
.test { margin-top: 0.75rem; border-top: 1px solid var(--border); padding-top: 0.75rem; }
</style>
