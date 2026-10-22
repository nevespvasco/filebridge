// Display helpers. Sizes use decimal units (GB = 10^9 bytes), like file managers do.

const UNITS = ["B", "KB", "MB", "GB", "TB"];

export function formatBytes(n: number | null | undefined, locale: string): string {
  if (n == null || !Number.isFinite(n)) return "—";
  let v = n;
  let u = 0;
  while (v >= 1000 && u < UNITS.length - 1) {
    v /= 1000;
    u++;
  }
  const digits = u === 0 ? 0 : v < 10 ? 2 : v < 100 ? 1 : 0;
  return `${v.toLocaleString(locale, { maximumFractionDigits: digits, minimumFractionDigits: digits })} ${UNITS[u]}`;
}

export function formatRate(bytesPerSec: number | null | undefined, locale: string): string {
  if (bytesPerSec == null || !Number.isFinite(bytesPerSec) || bytesPerSec <= 0) return "—";
  return `${formatBytes(bytesPerSec, locale)}/s`;
}

export function formatCount(n: number | null | undefined, locale: string): string {
  return n == null ? "—" : n.toLocaleString(locale);
}

export function formatDuration(secs: number | null | undefined): string {
  if (secs == null || !Number.isFinite(secs)) return "—";
  const s = Math.max(0, Math.round(secs));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const r = s % 60;
  if (h > 0) return `${h} h ${String(m).padStart(2, "0")} min`;
  if (m > 0) return `${m} min ${String(r).padStart(2, "0")} s`;
  return `${r} s`;
}

/** Service timestamps are Unix milliseconds. */
export function formatDate(ms: number | null | undefined, locale: string): string {
  if (ms == null) return "—";
  return new Date(ms).toLocaleString(locale, { dateStyle: "short", timeStyle: "short" });
}

export function percent(done: number, total: number): number {
  if (!total) return 0;
  return Math.min(100, Math.max(0, (done / total) * 100));
}

/** `[0x61, 0xcc, 0x81]` → `61 cc 81`. */
export function hexBytes(bytes: number[]): string {
  return bytes.map((b) => b.toString(16).padStart(2, "0")).join(" ");
}

/** `s` without any of `chars` at its end. A loop, not `/[…]+$/`: that regex backtracks. */
export function trimEnd(s: string, chars: string): string {
  let end = s.length;
  while (end > 0 && chars.includes(s[end - 1])) end--;
  return s.slice(0, end);
}

/** `s` without any of `chars` at either end. */
export function trimBoth(s: string, chars: string): string {
  let start = 0;
  while (start < s.length && chars.includes(s[start])) start++;
  return trimEnd(s.slice(start), chars);
}

/** The last path component, for compact lists. */
export function baseName(path: string): string {
  const trimmed = trimEnd(path, "/\\");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return i >= 0 ? trimmed.slice(i + 1) : trimmed;
}

/** The folder a path is in ("/home/demo" for "/home/demo/Fotos"); "" for a bare name. */
export function parentPath(path: string): string {
  const trimmed = trimEnd(path, "/\\");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return i > 0 ? trimmed.slice(0, i) : i === 0 ? trimmed.slice(0, 1) : "";
}

/** Minutes since midnight → `HH:MM` (the value of an `<input type="time">`). */
export function minutesToTime(m: number): string {
  return `${String(Math.floor(m / 60)).padStart(2, "0")}:${String(m % 60).padStart(2, "0")}`;
}

/** `HH:MM` → minutes since midnight; `null` if it isn't a time. */
export function timeToMinutes(t: string): number | null {
  const m = /^(\d{1,2}):(\d{2})$/.exec(t.trim());
  if (!m) return null;
  const [h, min] = [Number(m[1]), Number(m[2])];
  return h < 24 && min < 60 ? h * 60 + min : null;
}

/** A Unix-ms instant as a local time of day, e.g. `22:00`. */
export function formatTime(ms: number, locale: string): string {
  return new Date(ms).toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" });
}
