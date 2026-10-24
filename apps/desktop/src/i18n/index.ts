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

export function setLocale(locale: Locale) {
  i18n.global.locale.value = locale;
  // The full tag: a screen reader picks European Portuguese, and hyphenation follows it.
  document.documentElement.lang = bcp47(locale);
}

/** `pt` → `pt-PT` for number and date formatting. */
export function bcp47(locale: string): string {
  return locale === "pt" ? "pt-PT" : "en-GB";
}
