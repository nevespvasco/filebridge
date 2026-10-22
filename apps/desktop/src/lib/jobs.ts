// How a job is named on screen (docs/design/specs/D06, "Label"), and where it is continued.
import type { Router } from "vue-router";
import type { JobSummary_Serialize } from "@/bindings";
import { i18n } from "@/i18n";
import { useWizard } from "@/stores/wizard";
import { baseName } from "./format";

/** Opens an unsent job where it is decided and confirmed: a Repair job in its own page (its
 *  folder can't change, and the renames wait for that page's confirmation), any other in the New
 *  transfer wizard. */
export async function continueJob(job: JobSummary_Serialize, router: Router): Promise<void> {
  if (job.kind === "repair") {
    await router.push({ name: "repair", params: { id: job.id } });
    return;
  }
  await useWizard().openJob(job);
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
