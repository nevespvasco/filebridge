<script setup lang="ts">
// "Trust this server?" (ADR 0015): before a wizard first uses an SSH server, its host key is
// checked against known_hosts. A server known there goes straight through; an unknown one shows
// its key type and SHA-256 fingerprint, and is added only when the user confirms; one whose key
// changed is an error with no way to accept it here.
// Use: `await prompt.value.ensureKnown(host, port, knownHostsFile)` → true to go on, or
// `check(…)` to also learn whether the key was trusted just now (to retry what failed).
import { onBeforeUnmount, ref, shallowRef } from "vue";
import type { HostKeyView_Serialize } from "@/bindings";
import { api, errorMessage } from "@/lib/api";
import AppIcon from "./AppIcon.vue";
import ModalDialog from "./ModalDialog.vue";

const check = shallowRef<HostKeyView_Serialize | null>(null);
const busy = ref(false);
const error = ref<string | null>(null);
let settle: ((trusted: boolean) => void) | null = null;

/** Where the server's key stands once the user has answered: `known` (it already was in
 *  known_hosts), `trusted` (added just now) or `refused` (not trusted, or its key changed).
 *  Throws when the server can't be reached. */
async function checkKey(host: string, port: number, knownHostsFile: string | null): Promise<"known" | "trusted" | "refused"> {
  finish(false);
  const found = await api.scanHostKey(host, port, knownHostsFile);
  if (found.status === "known") return "known";
  error.value = null;
  check.value = found;
  const trusted = await new Promise<boolean>((resolve) => (settle = resolve));
  return trusted ? "trusted" : "refused";
}

/** Resolves `true` once the server's key is in known_hosts (already, or trusted now), `false`
 *  when the user doesn't trust it or its key changed. Throws when the server can't be reached. */
async function ensureKnown(host: string, port: number, knownHostsFile: string | null): Promise<boolean> {
  return (await checkKey(host, port, knownHostsFile)) !== "refused";
}

function finish(trusted: boolean) {
  check.value = null;
  error.value = null;
  settle?.(trusted);
  settle = null;
}

async function trust() {
  const c = check.value;
  if (!c?.fingerprint) return;
  busy.value = true;
  error.value = null;
  try {
    await api.trustHostKey(c.host, c.port, c.known_hosts_file, c.fingerprint);
    finish(true);
  } catch (e) {
    error.value = errorMessage(e);
  } finally {
    busy.value = false;
  }
}

/** The host key file's name part for a key type: `ssh-ed25519` → `ed25519`. */
function keyFileType(type: string | null): string {
  if (type?.startsWith("ecdsa")) return "ecdsa";
  if (type === "ssh-rsa" || type?.startsWith("rsa-")) return "rsa";
  return "ed25519";
}

onBeforeUnmount(() => finish(false));
defineExpose({ ensureKnown, check: checkKey });
</script>

<template>
  <ModalDialog
    :open="!!check"
    :title="check?.status === 'changed' ? $t('hostKey.changedTitle') : $t('hostKey.title')"
    role="alertdialog"
    :width="520"
    :busy="busy"
    @close="finish(false)"
  >
    <template #title>
      <span v-if="check?.status === 'changed'" class="title error"><AppIcon name="warning" :size="20" /> {{ $t("hostKey.changedTitle") }}</span>
      <span v-else class="title"><AppIcon name="key" :size="20" /> {{ $t("hostKey.title") }}</span>
    </template>
    <template v-if="check">
        <template v-if="check.status === 'unknown'">
          <p>{{ $t("hostKey.intro", { host: check.host }) }}</p>
          <dl class="key">
            <dt>{{ $t("hostKey.server") }}</dt>
            <dd class="mono">{{ check.port === 22 ? check.host : `${check.host}:${check.port}` }}</dd>
            <dt>{{ $t("hostKey.type") }}</dt>
            <dd class="mono">{{ check.key_type }}</dd>
            <dt>{{ $t("hostKey.fingerprint") }}</dt>
            <dd class="mono fp">{{ check.fingerprint }}</dd>
          </dl>
          <p class="small">{{ $t("hostKey.howToCheck") }}</p>
          <code class="cmd small">ssh-keygen -lf /etc/ssh/ssh_host_{{ keyFileType(check.key_type) }}_key.pub</code>
          <p class="small muted">{{ $t("hostKey.savedIn", { file: check.known_hosts_file }) }}</p>
        </template>
        <template v-else>
          <p>{{ $t("hostKey.changed", { host: check.host, file: check.known_hosts_file, line: check.line ?? "?" }) }}</p>
          <dl v-if="check.fingerprint" class="key">
            <dt>{{ $t("hostKey.presented") }}</dt>
            <dd class="mono fp">{{ check.key_type }} {{ check.fingerprint }}</dd>
          </dl>
          <p class="small">{{ $t("hostKey.changedWhat") }}</p>
        </template>
        <p v-if="error" class="error small">{{ error }}</p>
    </template>
    <template v-if="check" #actions>
          <button data-autofocus type="button" class="btn" :disabled="busy" @click="finish(false)">
            {{ check.status === "unknown" ? $t("common.cancel") : $t("hostKey.close") }}
          </button>
          <button v-if="check.status === 'unknown'" type="button" class="btn btn-primary" :disabled="busy" @click="trust">
            {{ busy ? $t("hostKey.trusting") : $t("hostKey.trust") }}
          </button>
    </template>
  </ModalDialog>
</template>

<style scoped>
.title { display: inline-flex; gap: var(--sp-2); align-items: center; }
p { margin: 0; }
.key { display: grid; grid-template-columns: auto 1fr; gap: 0.3rem 0.75rem; margin: 0.75rem 0; }
.key dt { color: var(--muted); }
.key dd { margin: 0; overflow-wrap: anywhere; user-select: all; }
.fp { font-weight: 600; }
.cmd { display: block; padding: 0.4rem 0.6rem; border: 1px solid var(--border); border-radius: var(--radius-sm); overflow-wrap: anywhere; user-select: all; }
</style>
