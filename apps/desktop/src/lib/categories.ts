// How a file Category looks: its icon and its colour token (docs/design/01-design-system.md,
// "File categories"). The Category itself comes from the engine (`FileCategory`).
import type { FileCategory } from "@/bindings";
import type { IconName } from "./icons";

export const CATEGORY_ICON: Record<FileCategory, IconName> = {
  image: "fileImage",
  video: "fileVideo",
  audio: "fileAudio",
  document: "fileDocument",
  archive: "fileArchive",
  code: "fileCode",
  other: "file",
};

/** The Category's colour, for graphics only (never text). */
export function categoryColor(c: FileCategory): string {
  return `var(--cat-${c})`;
}
