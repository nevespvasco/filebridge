<script setup lang="ts">
import AppIcon from "@/components/AppIcon.vue";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef } from "vue";
import TourButton from "@/components/TourButton.vue";
import { useTour } from "@/stores/tour";
import { useI18n } from "vue-i18n";
import { useRouter } from "vue-router";
import type { Profile_Serialize } from "@/bindings";
import ClampText from "@/components/ClampText.vue";
import StatusIcon from "@/components/StatusIcon.vue";
import AccessDialog from "@/components/AccessDialog.vue";
import ConfirmDialog from "@/components/ConfirmDialog.vue";
import DestinationSpace from "@/components/DestinationSpace.vue";
import EmptyState from "@/components/EmptyState.vue";
import LoadError from "@/components/LoadError.vue";
import FolderBrowser from "@/components/FolderBrowser.vue";
import HelpTip from "@/components/HelpTip.vue";
import HostKeyPrompt from "@/components/HostKeyPrompt.vue";
import PreflightList from "@/components/PreflightList.vue";
import ProfileForm from "@/components/ProfileForm.vue";
import RowMenu, { type MenuItem } from "@/components/RowMenu.vue";
import SkeletonRows from "@/components/SkeletonRows.vue";
import { api, errorMessage } from "@/lib/api";
import { useConnection } from "@/stores/connection";
import { profileKind, profileWhere, sshHosts, sshKeyAction, useProfiles, type TestState } from "@/stores/profiles";

const profiles = useProfiles();
const conn = useConnection();
const { t } = useI18n();
const creating = ref(false);
/** The destination being edited, in place of its card. */
const editingId = ref<string | null>(null);
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

// The guide starts by itself once, when there is a destination to point at.
const tour = useTour();
onMounted(async () => {
  await profiles.load();
  void profiles.loadAllStorage();
  await nextTick();
  if (profiles.list.length) tour.maybeStart("destinations");
});
onBeforeUnmount(() => tour.stop("destinations"));

function onSaved(p: Profile_Serialize) {
  creating.value = false;
  editingId.value = null;
  void test(p);
}

/** Test and Browse stay on the card; the rest is in its ⋮ menu (docs/design/specs/D03). */
function menuItems(p: Profile_Serialize): MenuItem[] {
  const key = sshKey(p);
  const items: MenuItem[] = [
    { id: "edit", label: t("destinations.edit"), icon: "edit" },
    { id: "repair", label: t("repair.button"), icon: "renamed", disabled: checkingKeys.value[p.id], reason: t("hostKey.checking") },
    ...(key ? [{ id: "access", label: t(`access.${key}.button`), icon: key === "install" ? ("key" as const) : ("lock" as const) }] : []),
    { id: "delete", label: t("common.delete"), icon: "close", danger: true },
  ];
  return conn.offline ? items.map((i) => ({ ...i, disabled: true, reason: t("conn.disconnected") })) : items;
}
function onMenu(p: Profile_Serialize, id: string) {
  switch (id) {
    case "edit":
      editingId.value = p.id;
      return;
    case "repair":
      return void browse(p, (x) => (repairing.value = x));
    case "access":
      access.value = { profile: p, action: sshKey(p)! };
      return;
    case "delete":
      deleting.value = p;
      return;
  }
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
</script>

<template>
  <div>
    <header class="page-head">
      <div>
        <h1 class="title-row">{{ $t("destinations.title") }} <TourButton v-if="profiles.list.length" tour="destinations" /></h1>
        <p class="lead-text">{{ $t("destinations.intro") }}</p>
      </div>
      <button v-if="!creating && profiles.list.length" data-tour="dest-new" class="btn btn-primary" :disabled="conn.offline" :title="conn.offline ? $t('conn.disconnected') : undefined" @click="creating = true"><AppIcon name="plus" /> {{ $t("dest.new") }}</button>
    </header>
    <p v-if="error" class="error">{{ error }}</p>

    <ProfileForm v-if="creating" class="form" @saved="onSaved" @cancel="creating = false" />
    <SkeletonRows v-if="!profiles.loaded" :rows="2" class="card" />
    <!-- With the engine down, a list already read stays on screen, greyed. A failed read is never
         "no destinations yet". -->
    <LoadError v-else-if="profiles.loadError && !(conn.offline && profiles.list.length)" :message="$t('load.destinations')" :error="profiles.loadError" @retry="profiles.load()" />

    <EmptyState v-else-if="!profiles.list.length && !creating" icon="destinations" :title="$t('destinations.empty')" :hint="$t('destinations.emptyHint')">
      <button class="btn btn-primary" :disabled="conn.offline" @click="creating = true"><AppIcon name="plus" /> {{ $t("dest.new") }}</button>
    </EmptyState>

    <div class="list">
      <template v-for="(p, i) in profiles.list" :key="p.id">
      <ProfileForm v-if="editingId === p.id" :profile="p" @saved="onSaved" @cancel="editingId = null" />
      <section v-else class="card" :class="{ stale: conn.offline }" :data-tour="i === 0 ? 'dest-card' : undefined">
        <div class="head">
          <div class="who">
            <AppIcon :name="profileKind(p)" :size="20" class="kind" />
            <div class="who-text">
              <strong>{{ p.name }}</strong>
              <span class="meta">
                <span class="badge badge-muted">{{ $t(`profile.kinds.${profileKind(p)}`) }}</span>
                <span v-if="nextcloudMode(p)" class="small muted">{{ $t("destinations.ncMode", { mode: $t(`webdav.modeShort.${nextcloudMode(p)}`) }) }}</span>
              </span>
            </div>
          </div>
          <div class="actions" :data-tour="i === 0 ? 'dest-actions' : undefined">
            <button class="btn btn-small" :title="conn.offline ? $t('conn.disconnected') : $t('dest.testTip')" :disabled="profiles.tests[p.id]?.running || checkingKeys[p.id] || conn.offline" @click="test(p)">{{ $t("destinations.test") }}</button>
            <button class="btn btn-small" :title="conn.offline ? $t('conn.disconnected') : $t('destinations.browseTip')" :disabled="checkingKeys[p.id] || conn.offline" @click="browse(p, (x) => (browsing = x))"><AppIcon name="reveal" /> {{ $t("destinations.browse") }}</button>
            <RowMenu :items="menuItems(p)" :label="$t('destinations.menu', { name: p.name })" @select="onMenu(p, $event)" />
          </div>
        </div>
        <!-- What it is, then how it is doing: where and what system, one muted block; the space
             and the last test under one divider, each failure said once. -->
        <div class="where small muted">
          <p class="mono ellipsis" :title="profileWhere(p)">{{ profileWhere(p) }}</p>
          <p>
            {{ $t(`profile.family.${p.target.family}`) }} · {{ p.target.case_sensitive ? $t("profile.caseSensitive") : $t("profile.caseInsensitive") }}
            <HelpTip :text="$t('profile.help.target')" :label="$t('profile.target')" />
          </p>
        </div>
        <p v-if="sshKey(p) === 'remove'" class="small ok-text"><AppIcon name="key" :size="16" /> {{ $t("access.uses") }}</p>
        <div class="status">
          <DestinationSpace :profile-id="p.id" :hide-error="!!profiles.tests[p.id] && !profiles.tests[p.id].running && failed(profiles.tests[p.id])" :refresh="() => checkSpace(p)" />
          <p v-if="checkingKeys[p.id]" class="small muted">{{ $t("hostKey.checking") }}</p>
          <div v-if="profiles.tests[p.id]" class="test">
            <p v-if="profiles.tests[p.id].running" class="small muted">{{ $t("dest.testing") }}</p>
            <p v-else-if="profiles.tests[p.id].error" class="failed small"><StatusIcon status="fail" /><ClampText :text="profiles.tests[p.id].error!" :lines="3" /></p>
            <PreflightList
              v-else-if="profiles.tests[p.id].result"
              :views="[profiles.tests[p.id].result!.transport, ...(profiles.tests[p.id].result!.integration ? [profiles.tests[p.id].result!.integration!] : [])]"
            />
          </div>
        </div>
      </section>
      </template>
    </div>
    <FolderBrowser
      :target="browsing ? { kind: 'profile', id: browsing.id } : null"
      :title="$t('browse.title', { destination: browsing?.name ?? '' })"
      @close="browsing = null"
    />
    <FolderBrowser
      :target="repairing ? { kind: 'profile', id: repairing.id } : null"
      :title="$t('repair.pickTitle', { destination: repairing?.name ?? '' })"
      :choose-label="$t('repair.pick')"
      :into-label="$t('repair.checkIn')"
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
.title-row { display: flex; gap: var(--sp-2); align-items: center; }
.page-head .lead-text { margin-bottom: 0; }
.list { display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.75rem; }
.head { display: flex; justify-content: space-between; gap: 1rem; align-items: center; flex-wrap: wrap; }
.kind { flex: none; margin-top: 0.1rem; color: var(--text-2); }
/* The name on its own line, its kind and mode under it: the same at every width and name length. */
.who { display: flex; gap: 0.6rem; align-items: flex-start; flex: 1 1 14rem; min-width: 0; }
.who-text { display: grid; gap: 0.2rem; min-width: 0; }
.who strong { overflow-wrap: anywhere; }
.meta { display: flex; gap: 0.3rem 0.6rem; align-items: center; flex-wrap: wrap; }
.actions { display: flex; gap: 0.4rem; flex-wrap: wrap; }
.where { display: grid; gap: 0.15rem; margin: 0.5rem 0 0; }
.where p { margin: 0; }
.key-warning { margin: 0.5rem 0; }
.key-warning p { margin: 0 0 0.4rem; }
.status { display: grid; gap: 0.6rem; margin-top: 0.75rem; border-top: 1px solid var(--border); padding-top: 0.75rem; }
.status p { margin: 0; }
.failed { display: flex; gap: 0.5rem; align-items: flex-start; }
</style>
