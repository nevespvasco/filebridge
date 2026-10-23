<script setup lang="ts">
// An S3 bucket's part of the destination form (ADR 0022): AWS or a compatible server (MinIO,
// Ceph, Garage…). The access key id names the key and stays in the profile; the secret access
// key is kept apart and goes to the system keyring, never into the profile.
import { computed } from "vue";
import AppIcon from "./AppIcon.vue";
import HelpTip from "./HelpTip.vue";

export type S3Form = {
  endpoint: string;
  region: string;
  provider: string;
  bucket: string;
  accessKeyId: string;
  pathStyle: boolean;
};

const fields = defineModel<S3Form>("fields", { required: true });
/** Cleared by the form once it is in the keyring. */
const secret = defineModel<string>("secret", { required: true });

/** rclone's names for the servers it knows apart; anything else works as "Other". */
const PROVIDERS = ["Other", "AWS", "Minio", "Ceph", "Cloudflare", "Wasabi", "Backblaze", "DigitalOcean", "Scaleway", "Hetzner"];

/** The bucket is its name alone; the folder inside it is the form's folder field. */
const bucketHasSlash = computed(() => fields.value.bucket.trim().replace(/^\/+|\/+$/g, "").includes("/"));
const plainHttp = computed(() => /^http:\/\//i.test(fields.value.endpoint.trim()) && !/^http:\/\/(localhost|127\.|\[::1\])/i.test(fields.value.endpoint.trim()));
</script>

<template>
  <div class="s3">
    <label class="field wide">
      <span class="label-line">{{ $t("s3.endpoint") }}
        <HelpTip :text="$t('s3.help.endpoint')" example="https://minio.example.com:9000" :label="$t('s3.endpoint')" />
      </span>
      <input v-model="fields.endpoint" spellcheck="false" autocomplete="url" placeholder="https://minio.example.com:9000" />
      <span v-if="plainHttp" class="hint warn-text">{{ $t("s3.https") }}</span>
      <span v-else class="hint">{{ $t("s3.hint.endpoint") }}</span>
    </label>
    <label class="field">
      <span class="label-line">{{ $t("s3.bucket") }}<span class="req" aria-hidden="true">*</span>
        <HelpTip :text="$t('s3.help.bucket')" example="fotos" :label="$t('s3.bucket')" />
      </span>
      <input v-model="fields.bucket" required spellcheck="false" placeholder="backup" />
      <span v-if="bucketHasSlash" class="hint error">{{ $t("s3.bucketSlash") }}</span>
      <span v-else class="hint">{{ $t("s3.hint.bucket") }}</span>
    </label>
    <label class="field">
      <span class="label-line">{{ $t("s3.region") }}
        <HelpTip :text="$t('s3.help.region')" example="eu-west-1 · us-east-1" :label="$t('s3.region')" />
      </span>
      <input v-model="fields.region" spellcheck="false" :placeholder="$t('common.optional')" />
      <span class="hint">{{ $t("s3.hint.region") }}</span>
    </label>
    <label class="field">
      <span class="label-line">{{ $t("s3.provider") }}
        <HelpTip :text="$t('s3.help.provider')" example="Minio" :label="$t('s3.provider')" />
      </span>
      <select v-model="fields.provider">
        <option v-for="p in PROVIDERS" :key="p" :value="p">{{ p === "Other" ? $t("s3.providerOther") : p }}</option>
      </select>
    </label>
    <label class="field">
      <span class="label-line">{{ $t("s3.accessKeyId") }}
        <HelpTip :text="$t('s3.help.accessKeyId')" :label="$t('s3.accessKeyId')" />
      </span>
      <input v-model="fields.accessKeyId" spellcheck="false" autocomplete="username" :placeholder="$t('s3.anonymous')" />
      <span class="hint">{{ $t("s3.hint.accessKeyId") }}</span>
    </label>
    <label v-if="fields.accessKeyId.trim()" class="field">
      <span class="label-line">{{ $t("s3.secret") }}<span class="req" aria-hidden="true">*</span>
        <HelpTip :text="$t('s3.help.secret')" :label="$t('s3.secret')" />
      </span>
      <input v-model="secret" type="password" autocomplete="new-password" spellcheck="false" />
      <span class="hint"><AppIcon name="lock" :size="16" /> {{ $t("profile.hint.password") }}</span>
    </label>
    <label class="field wide check">
      <span class="radio">
        <input v-model="fields.pathStyle" type="checkbox" />
        {{ $t("s3.pathStyle") }}
        <HelpTip :text="$t('s3.help.pathStyle')" example="minio.example.com/backup" :label="$t('s3.pathStyle')" />
      </span>
    </label>
  </div>
</template>

<style scoped>
/* Its fields sit in the form's grid. */
.s3 { display: contents; }
.wide { grid-column: 1 / -1; }
.radio { display: inline-flex; gap: 0.35rem; align-items: center; }
</style>
