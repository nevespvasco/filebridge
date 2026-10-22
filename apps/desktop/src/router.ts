import { createRouter, createWebHashHistory } from "vue-router";
import WizardView from "@/views/NewTransfer/WizardView.vue";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/home" },
    { path: "/home", name: "home", component: () => import("@/views/HomeView.vue") },
    { path: "/new", name: "new", component: WizardView },
    { path: "/jobs", name: "jobs", component: () => import("@/views/JobsView.vue") },
    { path: "/jobs/:id", name: "job", component: () => import("@/views/JobDetailView.vue"), props: true },
    { path: "/destinations", name: "destinations", component: () => import("@/views/DestinationsView.vue") },
    { path: "/settings", name: "settings", component: () => import("@/views/SettingsView.vue") },
  ],
});
