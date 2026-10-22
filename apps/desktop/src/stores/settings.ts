// Settings. Window preferences (language, notifications) are kept in this browser profile
// (localStorage); the app-wide bandwidth limit and send window live in the service, which applies
// them to every send (docs/adr/0010).
import { defineStore } from "pinia";
import { ref, watch } from "vue";
import type { AppSettings_Deserialize, AppSettings_Serialize } from "@/bindings";
import { setLocale, type Locale } from "@/i18n";
import { api, errorMessage } from "@/lib/api";

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

  /** The service's settings, as last read. */
  const app = ref<AppSettings_Serialize>({ bandwidth_limit: null, send_window: null });
  const appLoaded = ref(false);
  const appError = ref<string | null>(null);

  async function loadApp() {
    try {
      app.value = await api.getSettings();
      appError.value = null;
    } catch (e) {
      appError.value = errorMessage(e);
    } finally {
      appLoaded.value = true;
    }
  }

  /** Saves and re-reads them; throws the service's refusal. */
  async function saveApp(settings: AppSettings_Deserialize) {
    await api.saveSettings(settings);
    await loadApp();
  }

  return { locale, notifications, app, appLoaded, appError, loadApp, saveApp };
});
