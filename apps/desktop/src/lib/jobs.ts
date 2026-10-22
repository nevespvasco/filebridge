// How a job is named on screen (docs/design/specs/D06, "Label").
import type { JobSummary_Serialize } from "@/bindings";
import { baseName } from "./format";

/** The user's name, or the first source's name ("+N" for the others). */
export function jobLabel(job: Pick<JobSummary_Serialize, "label" | "sources">): string {
  if (job.label) return job.label;
  return defaultLabel(job.sources);
}

export function defaultLabel(sources: string[]): string {
  if (!sources.length) return "—";
  const first = baseName(sources[0]);
  return sources.length > 1 ? `${first} +${sources.length - 1}` : first;
}
