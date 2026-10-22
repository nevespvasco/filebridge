// How a job is named on screen (docs/design/specs/D06, "Label").
import type { JobSummary_Serialize } from "@/bindings";
import { i18n } from "@/i18n";
import { baseName } from "./format";

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
