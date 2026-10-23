<script setup lang="ts">
// An SMB share's part of the destination form (ADR 0022): the server, the share, the user and
// its domain. The password is kept apart: it goes to the system keyring, never into the profile.
import AppIcon from "./AppIcon.vue";
import HelpTip from "./HelpTip.vue";

export type SmbForm = { host: string; port: number; share: string; user: string; domain: string };

const fields = defineModel<SmbForm>("fields", { required: true });
/** Cleared by the form once it is in the keyring. */
const password = defineModel<string>("password", { required: true });
</script>

<template>
  <div class="smb">
    <label class="field">
      <span class="label-line">{{ $t("profile.host") }}<span class="req" aria-hidden="true">*</span>
        <HelpTip :text="$t('smb.help.host')" example="nas.local · 192.168.77.20" :label="$t('profile.host')" />
      </span>
      <input v-model="fields.host" required spellcheck="false" placeholder="nas.local" />
      <span class="hint">{{ $t("profile.hint.host") }}</span>
    </label>
    <label class="field">
      <span class="label-line">{{ $t("smb.share") }}<span class="req" aria-hidden="true">*</span>
        <HelpTip :text="$t('smb.help.share')" example="\\nas.local\backup → backup" :label="$t('smb.share')" />
      </span>
      <input v-model="fields.share" required spellcheck="false" placeholder="backup" />
      <span class="hint">{{ $t("smb.hint.share") }}</span>
    </label>
    <label class="field narrow">
      <span class="label-line">{{ $t("profile.port") }}
        <HelpTip :text="$t('smb.help.port')" example="445" :label="$t('profile.port')" />
      </span>
      <input v-model.number="fields.port" type="number" min="1" max="65535" />
      <span class="hint">{{ $t("smb.hint.port") }}</span>
    </label>
    <label class="field">
      <span class="label-line">{{ $t("profile.user") }}
        <HelpTip :text="$t('smb.help.user')" example="maria" :label="$t('profile.user')" />
      </span>
      <input v-model="fields.user" spellcheck="false" autocomplete="username" :placeholder="$t('common.optional')" />
      <span class="hint">{{ $t("smb.hint.user") }}</span>
    </label>
    <label class="field">
      <span class="label-line">{{ $t("smb.domain") }}
        <HelpTip :text="$t('smb.help.domain')" example="WORKGROUP" :label="$t('smb.domain')" />
      </span>
      <input v-model="fields.domain" spellcheck="false" placeholder="WORKGROUP" />
      <span class="hint">{{ $t("smb.hint.domain") }}</span>
    </label>
    <label v-if="fields.user.trim()" class="field">
      <span class="label-line">{{ $t("profile.password") }}
        <HelpTip :text="$t('smb.help.password')" :label="$t('profile.password')" />
      </span>
      <input v-model="password" type="password" autocomplete="new-password" spellcheck="false" />
      <span class="hint"><AppIcon name="lock" :size="16" /> {{ $t("profile.hint.password") }}</span>
    </label>
  </div>
</template>

<style scoped>
/* Its fields sit in the form's grid. */
.smb { display: contents; }
</style>
