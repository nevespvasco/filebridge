// Settings. Window preferences (language, notifications, theme, guides) are kept in this browser profile
// (localStorage); the app-wide bandwidth limit and send window live in the service, which applies
// them to every send (docs/adr/0010).
import { defineStore } from "pinia";
import { ref, watch } from "vue";
import type { AppSettings_Deserialize, AppSettings_Serialize } from "@/bindings";
import { setLocale, type Locale } from "@/i18n";
import { api, errorMessage } from "@/lib/api";

const KEY = "filebridge.settings";
/** Separate key: `public/theme-boot.js` reads it before the app loads. */
const THEME_KEY = "filebridge.theme";

export type Theme = "system" | "light" | "dark";
export const THEMES: Theme[] = ["system", "light", "dark"];

function loadTheme(): Theme {
  try {
    const t = localStorage.getItem(THEME_KEY);
    return t === "light" || t === "dark" ? t : "system";
  } catch {
    return "system";
  }
}

/** Light or Dark forces the tokens; System leaves them to `prefers-color-scheme`. */
function applyTheme(theme: Theme) {
  const root = document.documentElement;
  if (theme === "system") delete root.dataset.theme;
  else root.dataset.theme = theme;
}

/** First-run guides (docs/design/specs/D02): whether they start by themselves, and which
 *  version of each one was finished or skipped. */
export type Guides = { auto: boolean; seen: Record<string, number> };

type Stored = { locale: Locale; notifications: boolean; guides: Guides };

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
  const theme = ref<Theme>(loadTheme());
  const guides = ref<Guides>({ auto: stored.guides?.auto ?? true, seen: { ...(stored.guides?.seen ?? {}) } });

  function markTourSeen(id: string, version: number) {
    guides.value = { ...guides.value, seen: { ...guides.value.seen, [id]: version } };
  }
  /** "Show guides again": every guide starts by itself once more. */
  function resetGuides() {
    guides.value = { auto: true, seen: {} };
  }

  watch(
    theme,
    (t) => {
      applyTheme(t);
      try {
        if (t === "system") localStorage.removeItem(THEME_KEY);
        else localStorage.setItem(THEME_KEY, t);
      } catch {
        // Blocked storage: the choice lasts for this session only.
      }
    },
    { immediate: true },
  );

  watch(
    [locale, notifications, guides],
    () => {
      setLocale(locale.value);
      try {
        const s: Stored = { locale: locale.value, notifications: notifications.value, guides: guides.value };
        localStorage.setItem(KEY, JSON.stringify(s));
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

  return { locale, notifications, theme, guides, markTourSeen, resetGuides, app, appLoaded, appError, loadApp, saveApp };
});
