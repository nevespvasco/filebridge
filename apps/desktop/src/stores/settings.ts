// Window preferences, kept in this browser profile (localStorage). Service-side settings
// (bandwidth, schedule) are placeholders until the service exposes them.
import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { setLocale, type Locale } from "@/i18n";

const KEY = "filebridge.settings";

type Stored = { locale: Locale; notifications: boolean };

function load(): Partial<Stored> {
  try {
    return JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<Stored>;
  } catch {
    return {};
  }
}

export const useSettings = defineStore("settings", () => {
  const stored = load();
  const locale = ref<Locale>(stored.locale ?? (navigator.language.startsWith("pt") ? "pt" : "en"));
  const notifications = ref(stored.notifications ?? true);

  watch(
    [locale, notifications],
    () => {
      setLocale(locale.value);
      try {
        localStorage.setItem(KEY, JSON.stringify({ locale: locale.value, notifications: notifications.value }));
      } catch {
        // Private mode or blocked storage: the choice lasts for this session only.
      }
    },
    { immediate: true },
  );

  return { locale, notifications };
});
