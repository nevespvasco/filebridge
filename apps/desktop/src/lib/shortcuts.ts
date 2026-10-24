// The app's keyboard shortcuts (docs/design/02, "Keyboard"): one list, used by the key handler in
// App.vue, the sidebar's tooltips and Settings › Keyboard.

/** Ctrl/⌘+1…5 open these, in this order (the sidebar's places). Ctrl/⌘+N is "/new". */
export const SHORTCUT_PLACES = ["/", "/new", "/jobs", "/destinations", "/settings"];

/** The modifier as the keyboard shows it: ⌘ on a Mac, Ctrl elsewhere. */
export const modKey = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent) ? "⌘" : "Ctrl";

/** What each shortcut does, for Settings › Keyboard: i18n key of the place, and the key. */
export const SHORTCUTS: { key: string; label: string }[] = [
  { key: "N", label: "nav.new" },
  ...SHORTCUT_PLACES.map((path, i) => ({
    key: String(i + 1),
    label: { "/": "nav.home", "/new": "nav.new", "/jobs": "nav.jobs", "/destinations": "nav.destinations", "/settings": "nav.settings" }[path]!,
  })),
];
