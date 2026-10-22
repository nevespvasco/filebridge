// What an issue list decides through (docs/04, "Decisions model"): the New transfer wizard's job
// by default, or the job of the page that provides its own (a Repair job, `RepairView`).
import type { InjectionKey } from "vue";
import type { DecideManyResult, DecisionInput } from "@/bindings";

export interface Decisions {
  /** The rule whose issues are selected: one group at a time. */
  selectionGroup: string | null;
  decide(issueId: number, decision: DecisionInput): Promise<void>;
  decideAll(rule: string, decision: DecisionInput, query?: string | null): Promise<number>;
  decideMany(ids: number[], decision: DecisionInput): Promise<DecideManyResult>;
}

export const DECISIONS: InjectionKey<Decisions> = Symbol("decisions");
