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

/** The last path component, for compact lists. */
export function baseName(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return i >= 0 ? trimmed.slice(i + 1) : trimmed;
}
