import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { i18n } from "./i18n";
import { router } from "./router";
import "./style.css";

async function boot() {
  // `VITE_MOCK=1 pnpm dev` runs the UI in a plain browser against an in-memory fake service.
  if (import.meta.env.VITE_MOCK === "1") {
    const { installMock } = await import("./dev/mock");
    installMock();
  }
  createApp(App).use(createPinia()).use(i18n).use(router).mount("#app");
}

void boot();
