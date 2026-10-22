<script setup lang="ts">
import { trimEnd } from "@/lib/format";
import AppIcon from "@/components/AppIcon.vue";
import { computed, onMounted, ref } from "vue";
import type { Profile_Serialize } from "@/bindings";
import FolderBrowser from "@/components/FolderBrowser.vue";
import ProfileForm from "@/components/ProfileForm.vue";
import PreflightList from "@/components/PreflightList.vue";
import DetailsPanel from "@/components/DetailsPanel.vue";
import HelpTip from "@/components/HelpTip.vue";
import { useFmt } from "@/composables/useFmt";
import { request } from "@/lib/requests";
import { freeBytes, profileKind, profileWhere, useProfiles } from "@/stores/profiles";
import { useWizard } from "@/stores/wizard";

const wizard = useWizard();
const profiles = useProfiles();
const fmt = useFmt();
const creating = ref(false);

onMounted(async () => {
  await profiles.load();
  if (!profiles.list.length) creating.value = true;
  if (wizard.profileId && !profiles.tests[wizard.profileId]) void profiles.test(wizard.profileId);
});

function choose(p: Profile_Serialize) {
  // A folder of another destination means nothing here.
  if (wizard.profileId !== p.id) wizard.options = { ...wizard.options, target_dir: null };
  wizard.profileId = p.id;
  if (!profiles.tests[p.id]) void profiles.test(p.id);
}

function onSaved(p: Profile_Serialize) {
  creating.value = false;
  choose(p);
}

// Where in the destination this transfer goes (docs/design/specs/D11): the root, or a folder under it.
const browsing = ref(false);
const selectedProfile = computed(() => (wizard.profileId ? profiles.byId[wizard.profileId] : undefined));
function chooseFolder(path: string) {
  wizard.options = { ...wizard.options, target_dir: path || null };
  browsing.value = false;
}
const selectedTest = computed(() => (wizard.profileId ? profiles.tests[wizard.profileId] : undefined));
const failing = computed(() =>
  [selectedTest.value?.result?.transport, selectedTest.value?.result?.integration].some((v) => v?.checks.some((c) => c.status === "fail")),
);

function next() {
  void wizard.analyse();
  wizard.step = 3;
}

const details = computed(() => [
  request("list_profiles"),
  ...(wizard.profileId ? [request("test_profile", { id: wizard.profileId })] : []),
]);
</script>

<template>
  <div class="step">
    <h2>{{ $t("dest.title") }}</h2>
    <div class="guide">
      <AppIcon class="guide-icon" name="pointing" :size="20" />
      <p>{{ profiles.loaded && !profiles.list.length ? $t("dest.guideFirst") : $t("dest.guide") }}</p>
    </div>
    <p v-if="profiles.loadError" class="error">{{ profiles.loadError }}</p>

    <div class="profiles" role="radiogroup" :aria-label="$t('dest.title')">
      <button
        v-for="p in profiles.list"
        :key="p.id"
        type="button"
        role="radio"
        class="profile card"
        :aria-checked="wizard.profileId === p.id"
        @click="choose(p)"
      >
        <span class="top">
          <span class="kind badge badge-muted">{{ $t(`profile.kinds.${profileKind(p)}`) }}</span>
          <span v-if="wizard.profileId === p.id" class="badge badge-accent"><AppIcon name="check" /> {{ $t("dest.selected") }}</span>
        </span>
        <strong>{{ p.name }}</strong>
        <span class="small muted ellipsis mono" :title="profileWhere(p)">{{ profileWhere(p) }}</span>
        <span class="small">
          <template v-if="profiles.tests[p.id]?.running">{{ $t("dest.testing") }}</template>
          <template v-else-if="profiles.tests[p.id]?.error"><span class="error">{{ $t("dest.unreachable") }}</span></template>
          <template v-else-if="profiles.tests[p.id]?.result">
            {{ freeBytes(profiles.tests[p.id]?.result) != null ? $t("preflight.free", { size: fmt.bytes(freeBytes(profiles.tests[p.id]?.result)) }) : $t("preflight.freeUnknown") }}
          </template>
        </span>
      </button>
      <button v-if="!creating" type="button" class="profile card add" @click="creating = true">
        <strong><AppIcon name="plus" /> {{ $t("dest.new") }}</strong>
        <span class="small muted">{{ $t("dest.newHint") }}</span>
      </button>
    </div>

    <ProfileForm v-if="creating" class="form" @saved="onSaved" @cancel="creating = false" />

    <div v-if="selectedProfile && !creating" class="card into">
      <span class="small muted">{{ $t("step.dest.sendInto") }}</span>
      <span class="path">{{ trimEnd(selectedProfile.destination.root, "/") }}/{{ wizard.options.target_dir ?? "" }}</span>
      <button class="btn btn-small" @click="browsing = true"><AppIcon name="reveal" /> {{ $t("step.dest.change") }}</button>
      <button v-if="wizard.options.target_dir" class="btn btn-small" @click="chooseFolder('')">{{ $t("step.dest.root") }}</button>
    </div>
    <FolderBrowser
      :target="browsing && selectedProfile ? { kind: 'profile', id: selectedProfile.id } : null"
      :title="$t('browse.title', { destination: selectedProfile?.name ?? '' })"
      :start="wizard.options.target_dir"
      :choose-label="$t('browse.choose')"
      @choose="chooseFolder"
      @close="browsing = false"
    />

    <div v-if="selectedTest && !creating" class="card test">
      <div class="test-head">
        <strong>{{ $t("dest.test") }}<HelpTip :text="$t('dest.testTip')" :label="$t('dest.test')" /></strong>
        <button class="btn btn-small" :title="$t('dest.testTip')" :disabled="selectedTest.running" @click="profiles.test(wizard.profileId!)">{{ $t("dest.retest") }}</button>
      </div>
      <p v-if="selectedTest.running" class="muted small">{{ $t("dest.testing") }}</p>
      <p v-else-if="selectedTest.error" class="error small">{{ selectedTest.error }}</p>
      <PreflightList v-else-if="selectedTest.result" :views="[selectedTest.result.transport, ...(selectedTest.result.integration ? [selectedTest.result.integration] : [])]" />
      <p class="small muted">{{ $t("dest.etaLater") }}</p>
    </div>

    <DetailsPanel :lines="details" />

    <div class="footer">
      <button class="btn" @click="wizard.step = 1"><AppIcon name="back" /> {{ $t("common.back") }}</button>
      <span class="grow" />
      <span v-if="creating" class="why-disabled">{{ $t("dest.finishForm") }}</span>
      <span v-else-if="!wizard.profileId" class="why-disabled">{{ $t("dest.needOne") }}</span>
      <span v-else-if="failing" class="small warn-text">{{ $t("dest.failingHint") }}</span>
      <button class="btn btn-primary" :title="$t('dest.analyseTip')" :disabled="!wizard.canAnalyse || creating" @click="next">{{ $t("dest.analyse") }} <AppIcon name="next" /></button>
    </div>
  </div>
</template>

<style scoped>
.into { display: flex; gap: 0.6rem; align-items: center; flex-wrap: wrap; margin-top: 1rem; }
.into .path { flex: 1; min-width: min(12rem, 100%); }
.profiles { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 0.75rem; margin: 1rem 0; }
.profile {
  all: unset;
  box-sizing: border-box;
  cursor: pointer;
  display: grid;
  gap: 0.3rem;
  align-content: start;
  min-width: 0;
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 0.85rem 1rem;
  background: var(--surface);
}
.profile[aria-checked="true"] { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
.profile:focus-visible { outline: 2px solid var(--accent); }
.profile.add { border-style: dashed; }
.top { display: flex; gap: 0.4rem; justify-content: space-between; align-items: center; }
.profile[aria-checked="true"] { background: var(--accent-soft); }
.form, .test { margin-top: 1rem; }
.test-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.5rem; }
</style>
