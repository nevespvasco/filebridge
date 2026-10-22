// Window-side platform features: the file picker, drag & drop, OS notifications. They come from
// Tauri plugins and `@tauri-apps/api`, not from the service.
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

export const isMock = import.meta.env.VITE_MOCK === "1";

export type PickOptions = { directory: boolean; multiple?: boolean; title?: string };

/** Mock mode only: a browser can't open the OS picker, so `src/dev/mock.ts` puts one here. */
let mockPicker: ((opts: PickOptions) => Promise<string[]>) | null = null;
export function setMockPicker(picker: (opts: PickOptions) => Promise<string[]>) {
  mockPicker = picker;
}

/** Native picker. Returns absolute paths (empty when cancelled). */
export async function pickPaths(opts: PickOptions): Promise<string[]> {
  if (isMock) return mockPicker ? mockPicker(opts) : [];
  const picked = await open({
    directory: opts.directory,
    multiple: opts.multiple ?? true,
    title: opts.title,
  });
  if (picked == null) return [];
  return Array.isArray(picked) ? picked : [picked];
}

/** Mock mode only: where "Save as" answers from. */
let mockSave: ((defaultName: string) => Promise<string | null>) | null = null;
export function setMockSave(fn: (defaultName: string) => Promise<string | null>) {
  mockSave = fn;
}

/** The OS "Save as" dialog; `null` when cancelled. */
export async function pickSavePath(defaultName: string, extension: string, filterName: string): Promise<string | null> {
  if (isMock) return mockSave ? mockSave(defaultName) : null;
  return save({ defaultPath: defaultName, filters: [{ name: filterName, extensions: [extension] }] });
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

/** Opens the OS file manager with `path` selected. Reads only: nothing is opened or changed. */
export async function revealInFileManager(path: string): Promise<void> {
  if (isMock) {
    console.info(`[reveal] ${path}`);
    return;
  }
  await revealItemInDir(path);
}

/** Copies text to the clipboard. */
export async function copyText(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}
