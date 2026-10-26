import { createI18n } from "vue-i18n";
import en from "./en.json";
import pt from "./pt.json";

export type Locale = "pt" | "en";
export const LOCALES: Locale[] = ["pt", "en"];

export const i18n = createI18n({
  legacy: false,
  locale: "pt",
  fallbackLocale: "en",
  messages: { pt, en },
});

/** On a phone there is no window and no separate service: the texts under `phone` (same keys
 *  as the rest of the file) replace the ones that say otherwise. On an iPhone, those under
 *  `iphone` then replace the ones about sending in the background, which iOS doesn't allow. */
export function applyPhoneTexts(iphone = false) {
  for (const [locale, m] of Object.entries({ pt, en })) {
    i18n.global.mergeLocaleMessage(locale, m.phone);
    if (iphone) i18n.global.mergeLocaleMessage(locale, m.iphone);
  }
}

export function setLocale(locale: Locale) {
  i18n.global.locale.value = locale;
  // The full tag: a screen reader picks European Portuguese, and hyphenation follows it.
  document.documentElement.lang = bcp47(locale);
}

/** `pt` → `pt-PT` for number and date formatting. */
export function bcp47(locale: string): string {
  return locale === "pt" ? "pt-PT" : "en-GB";
}
