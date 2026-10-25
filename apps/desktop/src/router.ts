import { createRouter, createWebHashHistory } from "vue-router";
import WizardView from "@/views/NewTransfer/WizardView.vue";
import { useWizard } from "@/stores/wizard";

export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/home" },
    { path: "/home", name: "home", component: () => import("@/views/HomeView.vue") },
    {
      path: "/new",
      name: "new",
      component: WizardView,
      // "New transfer" starts a clean one once the last was sent; its progress lives in its page.
      beforeEnter: () => {
        const wizard = useWizard();
        if (wizard.step === 5) wizard.reset();
      },
    },
    { path: "/jobs", name: "jobs", component: () => import("@/views/JobsView.vue") },
    { path: "/jobs/:id", name: "job", component: () => import("@/views/JobDetailView.vue"), props: true },
    { path: "/compare/:a/:b?", name: "compare", component: () => import("@/views/CompareView.vue"), props: true },
    { path: "/repair/:id", name: "repair", component: () => import("@/views/RepairView.vue"), props: true },
    { path: "/schedules", name: "schedules", component: () => import("@/views/SchedulesView.vue") },
    { path: "/destinations", name: "destinations", component: () => import("@/views/DestinationsView.vue") },
    { path: "/settings", name: "settings", component: () => import("@/views/SettingsView.vue") },
  ],
});
