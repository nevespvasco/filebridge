// Window-side platform features: the file picker, drag & drop, OS notifications. They come from
// Tauri plugins and `@tauri-apps/api`, not from the service.
import { open } from "@tauri-apps/plugin-dialog";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { getCurrentWebview } from "@tauri-apps/api/webview";

export const isMock = import.meta.env.VITE_MOCK === "1";

/** Native picker. Returns absolute paths (empty when cancelled). */
export async function pickPaths(opts: { directory: boolean; multiple?: boolean; title?: string }): Promise<string[]> {
  if (isMock) {
    return opts.directory ? ["/home/demo/Fotos"] : ["/home/demo/Documentos/relatório.pdf"];
  }
  const picked = await open({
    directory: opts.directory,
    multiple: opts.multiple ?? true,
    title: opts.title,
  });
  if (picked == null) return [];
  return Array.isArray(picked) ? picked : [picked];
}

export type DropState = "over" | "drop" | "leave";

/** Files and folders dropped on the window. Returns an unsubscribe function. */
export async function onFileDrop(cb: (state: DropState, paths: string[]) => void): Promise<() => void> {
  if (isMock) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    if (p.type === "enter" || p.type === "over") cb("over", []);
    else if (p.type === "drop") cb("drop", p.paths);
    else cb("leave", []);
  });
}

let permission: boolean | null = null;

/** OS notification; silently skipped when the user refused them. */
export async function notify(title: string, body: string): Promise<void> {
  if (isMock) {
    console.info(`[notification] ${title}: ${body}`);
    return;
  }
  if (permission === null) {
    permission = await isPermissionGranted();
    if (!permission) permission = (await requestPermission()) === "granted";
  }
  if (permission) sendNotification({ title, body });
}
