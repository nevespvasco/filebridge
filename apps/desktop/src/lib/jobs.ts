// How a job is named on screen (docs/design/specs/D06, "Label"), and where it is continued.
import type { Router } from "vue-router";
import type { JobSummary_Serialize } from "@/bindings";
import { i18n } from "@/i18n";
import { useConfirm } from "@/stores/confirm";
import { useWizard } from "@/stores/wizard";
import { baseName } from "./format";

/** Before the wizard is reused for something else: when it holds choices not sent yet, asks
 *  whether to drop them. Resolves `true` to go on. */
export async function discardWizardIfAsked(): Promise<boolean> {
  if (!useWizard().hasWork) return true;
  const t = i18n.global.t;
  return useConfirm().ask({
    title: t("wizard.discard.title"),
    consequences: [t("wizard.discard.forgets"), t("wizard.discard.noDelete")],
    confirmLabel: t("wizard.discard.confirm"),
    cancelLabel: t("wizard.discard.keep"),
  });
}

/** Opens an unsent job where it is decided and confirmed: a Repair job in its own page (its
 *  folder can't change, and the renames wait for that page's confirmation), any other in the New
 *  transfer wizard. */
export async function continueJob(job: JobSummary_Serialize, router: Router): Promise<void> {
  if (job.kind === "repair") {
    await router.push({ name: "repair", params: { id: job.id } });
    return;
  }
  const wizard = useWizard();
  if (wizard.jobId !== job.id) {
    if (!(await discardWizardIfAsked())) return;
    await wizard.openJob(job);
  }
  await router.push({ name: "new" });
}

type Labelled = Pick<JobSummary_Serialize, "label" | "sources"> & Partial<Pick<JobSummary_Serialize, "kind" | "options">>;

/** The user's name; for a Repair job, "Repair · <folder>"; else the first source's name ("+N"
 *  for the others). */
export function jobLabel(job: Labelled): string {
  if (job.label) return job.label;
  if (job.kind === "repair") return repairLabel(job.options?.target_dir ?? null);
  return defaultLabel(job.sources);
}

/** "Repair · Fotos", or "Repair · whole destination". */
export function repairLabel(dir: string | null): string {
  const t = i18n.global.t;
  return t("repair.label", { folder: dir ? baseName(dir) : t("repair.root") });
}

export function defaultLabel(sources: string[]): string {
  if (!sources.length) return "—";
  const first = baseName(sources[0]);
  return sources.length > 1 ? `${first} +${sources.length - 1}` : first;
}
