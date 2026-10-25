// First-run guides (docs/design/specs/D02, "Guided tour"): each one is data, a list of steps that
// point at elements marked `data-tour="…"`. Bumping a guide's `version` shows it again to people
// who already saw it, after it changes.
import { SEND_STEPS } from "@/stores/wizard";

export type Placement = "top" | "bottom" | "left" | "right";

export type TourStep = {
  /** The `data-tour` value of the element it points at. */
  target: string;
  /** i18n keys. */
  title: string;
  body: string;
  /** Values for the title's and body's placeholders. */
  params?: Record<string, unknown>;
  placement?: Placement;
  /** Left out when its element isn't on screen (e.g. no recent sources yet). */
  optional?: boolean;
};

export type Tour = { version: number; steps: TourStep[] };

export const TOURS = {
  source: {
    version: 1,
    steps: [
      { target: "wizard-stepper", title: "tour.source.steps.title", body: "tour.source.steps.body", params: { n: SEND_STEPS }, placement: "bottom" },
      { target: "source-add", title: "tour.source.add.title", body: "tour.source.add.body", placement: "bottom" },
      { target: "source-recent", title: "tour.source.recent.title", body: "tour.source.recent.body", placement: "top", optional: true },
      { target: "source-chosen", title: "tour.source.chosen.title", body: "tour.source.chosen.body", placement: "top", optional: true },
      { target: "source-next", title: "tour.source.next.title", body: "tour.source.next.body", placement: "top" },
    ],
  },
  analysis: {
    version: 1,
    steps: [
      { target: "analysis-summary", title: "tour.analysis.summary.title", body: "tour.analysis.summary.body", placement: "bottom" },
      { target: "analysis-decide", title: "tour.analysis.decide.title", body: "tour.analysis.decide.body", placement: "bottom", optional: true },
      { target: "analysis-names", title: "tour.analysis.names.title", body: "tour.analysis.names.body", placement: "top" },
      { target: "analysis-next", title: "tour.analysis.next.title", body: "tour.analysis.next.body", placement: "top" },
    ],
  },
  destinations: {
    version: 1,
    steps: [
      { target: "dest-card", title: "tour.destinations.card.title", body: "tour.destinations.card.body", placement: "bottom" },
      { target: "dest-actions", title: "tour.destinations.actions.title", body: "tour.destinations.actions.body", placement: "left" },
      { target: "dest-new", title: "tour.destinations.new.title", body: "tour.destinations.new.body", placement: "bottom", optional: true },
    ],
  },
} satisfies Record<string, Tour>;

export type TourId = keyof typeof TOURS;

export function tourTarget(step: TourStep): HTMLElement | null {
  return document.querySelector<HTMLElement>(`[data-tour="${step.target}"]`);
}

export type Rect = { top: number; left: number; width: number; height: number };
export type Size = { width: number; height: number };

/** Below this width the popover sits at the bottom of the window, full width. */
export const NARROW = 560;

const OPPOSITE: Record<Placement, Placement> = { top: "bottom", bottom: "top", left: "right", right: "left" };

/**
 * Where the popover goes next to `target` (viewport coordinates): the preferred side, else the
 * opposite one, else the others; clamped inside the window. Centred without a target.
 */
export function place(target: Rect | null, pop: Size, view: Size, preferred: Placement = "bottom", gap = 12, margin = 12): { top: number; left: number; placement: Placement | "center" } {
  const clampX = (x: number) => Math.max(margin, Math.min(x, view.width - pop.width - margin));
  const clampY = (y: number) => Math.max(margin, Math.min(y, view.height - pop.height - margin));
  if (!target) return { top: clampY((view.height - pop.height) / 2), left: clampX((view.width - pop.width) / 2), placement: "center" };
  if (view.width < NARROW) {
    // Bottom of the window, unless the target is down there: then the top. Full width (`.narrow
    // .pop`), so the left edge is the margin, whatever width the popover had when measured.
    const low = target.top + target.height / 2 > view.height / 2;
    return { top: low ? margin : view.height - pop.height - margin, left: margin, placement: low ? "top" : "bottom" };
  }
  const at = (p: Placement) => {
    switch (p) {
      case "top":
        return { top: target.top - gap - pop.height, left: target.left + target.width / 2 - pop.width / 2 };
      case "bottom":
        return { top: target.top + target.height + gap, left: target.left + target.width / 2 - pop.width / 2 };
      case "left":
        return { top: target.top + target.height / 2 - pop.height / 2, left: target.left - gap - pop.width };
      case "right":
        return { top: target.top + target.height / 2 - pop.height / 2, left: target.left + target.width + gap };
    }
  };
  const fits = (p: Placement) => {
    const { top, left } = at(p);
    return p === "top" || p === "bottom"
      ? top >= margin && top + pop.height <= view.height - margin
      : left >= margin && left + pop.width <= view.width - margin;
  };
  const order: Placement[] = [preferred, OPPOSITE[preferred], ...(["bottom", "top", "right", "left"] as Placement[]).filter((p) => p !== preferred && p !== OPPOSITE[preferred])];
  const chosen = order.find(fits) ?? preferred;
  const { top, left } = at(chosen);
  return { top: clampY(top), left: clampX(left), placement: chosen };
}
