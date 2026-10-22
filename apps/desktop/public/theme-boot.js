// Applies the chosen theme before the first paint, so a dark-theme user never sees a white flash.
// Kept in sync with `stores/settings.ts` (same key and values). A plain file, not inline: the
// window's CSP allows only same-origin scripts.
try {
  var theme = localStorage.getItem("filebridge.theme");
  if (theme === "light" || theme === "dark") document.documentElement.dataset.theme = theme;
} catch (e) {
  // Storage blocked: follow the system.
}
