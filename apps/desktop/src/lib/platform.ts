// Window-side platform features: the file picker, drag & drop, OS notifications. They come from
// Tauri plugins and `@tauri-apps/api`, not from the service. On a phone (ADR 0013) the folder
// picker is the app's own Android or iOS plugin, and drag & drop, the tray and "show in the file
// manager" don't exist.
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { commands, type Platform } from "@/bindings";
import { i18n, applyPhoneTexts } from "@/i18n";
import { api } from "@/lib/api";

export const isMock = import.meta.env.VITE_MOCK === "1";

let current: Platform = "desktop";

/** Asks Rust where the app runs; called once before the app mounts. */
export async function initPlatform(): Promise<void> {
  if (!isMock) current = await commands.platform();
  if (isMobile()) applyPhoneTexts(current === "ios");
}

/** Android or iOS: no tray, no service, no drag & drop, folders only. */
export function isMobile(): boolean {
  return current !== "desktop";
}

/** Whether the disks mounted on the computer are offered as sources (not on a phone: its
 *  storage is one place, reached with the folder picker). */
export function canListVolumes(): boolean {
  return !isMobile();
}

/** Whether "show in the file manager" exists here (not on a phone). */
export function canReveal(): boolean {
  return !isMobile();
}

export type PickOptions = { directory: boolean; multiple?: boolean; title?: string; defaultPath?: string };

/** Mock mode only: a browser can't open the OS picker, so `src/dev/mock.ts` puts one here. */
let mockPicker: ((opts: PickOptions) => Promise<string[]>) | null = null;
export function setMockPicker(picker: (opts: PickOptions) => Promise<string[]>) {
  mockPicker = picker;
}

/** Native picker. Returns absolute paths (empty when cancelled). On a phone only folders can
 *  be picked, one at a time; on Android after "All files access" is granted. */
export async function pickPaths(opts: PickOptions): Promise<string[]> {
  if (isMock) return mockPicker ? mockPicker(opts) : [];
  if (isMobile()) return pickPhoneFolder(opts);
  const picked = await open({
    directory: opts.directory,
    multiple: opts.multiple ?? true,
    title: opts.title,
    defaultPath: opts.defaultPath,
  });
  if (picked == null) return [];
  return Array.isArray(picked) ? picked : [picked];
}

async function pickPhoneFolder(opts: PickOptions): Promise<string[]> {
  if (!opts.directory) return [];
  if (!(await api.storageAccess()) && !(await api.requestStorageAccess())) {
    throw new Error(i18n.global.t("mobile.storageDenied"));
  }
  const path = await api.pickFolder();
  return path ? [path] : [];
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
  if (isMock) return mockFileDrop(cb);
  if (isMobile()) return () => {};
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    if (p.type === "enter" || p.type === "over") cb("over", []);
    else if (p.type === "drop") cb("drop", p.paths);
    else cb("leave", []);
  });
}

/** Mock mode only: the page's own drag & drop. A browser hides real paths, so a dropped item
 *  becomes `/home/demo/<its name>`. */
function mockFileDrop(cb: (state: DropState, paths: string[]) => void): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const hasFiles = (e: DragEvent) => !!e.dataTransfer?.types.includes("Files");
  // `dragleave` fires between child elements too: "left" is when `dragover` stops coming.
  const onOver = (e: DragEvent) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    cb("over", []);
    clearTimeout(timer);
    timer = setTimeout(() => cb("leave", []), 200);
  };
  const onDrop = (e: DragEvent) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    clearTimeout(timer);
    const names = [...(e.dataTransfer?.files ?? [])].map((f) => f.name);
    cb("drop", names.map((n) => `/home/demo/${n}`));
  };
  window.addEventListener("dragover", onOver);
  window.addEventListener("drop", onDrop);
  return () => {
    clearTimeout(timer);
    window.removeEventListener("dragover", onOver);
    window.removeEventListener("drop", onDrop);
  };
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
  if (!canReveal()) return;
  if (isMock) {
    console.info(`[reveal] ${path}`);
    return;
  }
  await revealItemInDir(path);
}

/** Opens an https page in the system browser (e.g. the server's own settings). */
export async function openInBrowser(url: string): Promise<void> {
  if (!/^https:\/\//i.test(url)) return;
  if (isMock) {
    window.open(url, "_blank", "noopener");
    return;
  }
  await openUrl(url);
}

/** Phones: asks Android, once, not to stop filebridge to save battery while it sends. */
export async function askBatteryExemptionOnce(): Promise<void> {
  if (isMock || current !== "android") return;
  const key = "filebridge.batteryAsked";
  try {
    if (localStorage.getItem(key)) return;
    localStorage.setItem(key, "1");
  } catch {
    // No storage: ask every time rather than never.
  }
  try {
    await api.requestBatteryExemption();
  } catch (e) {
    console.warn("battery exemption:", e);
  }
}

/** Copies text to the clipboard. */
export async function copyText(text: string): Promise<void> {
  await navigator.clipboard.writeText(text);
}
