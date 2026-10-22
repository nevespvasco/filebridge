// How an entry's status looks (docs/design/specs/D05, "Status: what the user sees"). The status
// itself comes from the engine (`EntryStatus`).
import type { EntryStatus } from "@/bindings";
import type { IconName } from "./icons";

export const STATUS_LOOK: Record<EntryStatus, { icon: IconName; tone: "muted" | "accent" | "ok" | "warn" | "danger" }> = {
  not_sent: { icon: "pending", tone: "muted" },
  sending: { icon: "sending", tone: "accent" },
  sent: { icon: "check", tone: "ok" },
  verified: { icon: "verified", tone: "ok" },
  different: { icon: "different", tone: "danger" },
  missing: { icon: "missing", tone: "danger" },
  unverifiable: { icon: "warnCircle", tone: "warn" },
  failed: { icon: "failCircle", tone: "danger" },
  skipped: { icon: "skipped", tone: "muted" },
};

export const PROBLEM_STATUSES: EntryStatus[] = ["different", "missing", "unverifiable", "failed"];
