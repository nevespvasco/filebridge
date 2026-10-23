// Previews of source images for the Files tab and the Entry detail (ADR 0027). The service makes
// and caches them on disk; this keeps the recent ones in memory and asks for few at a time,
// newest request first, so scrolling a long list never floods the service.
import { api } from "./api";

/** Previews kept in memory (data URLs of about 15 kB, or `null` for "none"). */
const KEEP = 400;
/** Requests in flight at once. */
const AT_ONCE = 3;

const known = new Map<string, string | null>();
type Waiting = { jobId: string; idx: number; done: (url: string | null) => void; cancelled: boolean };
const queue: Waiting[] = [];
let running = 0;

const keyOf = (jobId: string, idx: number) => `${jobId}:${idx}`;

/** The preview already loaded: a data URL, `null` for none, `undefined` when not asked yet. */
export function knownThumbnail(jobId: string, idx: number): string | null | undefined {
  const key = keyOf(jobId, idx);
  const v = known.get(key);
  if (v !== undefined) {
    // Most recently used last, so the oldest go first.
    known.delete(key);
    known.set(key, v);
  }
  return v;
}

function remember(key: string, url: string | null) {
  known.set(key, url);
  while (known.size > KEEP) known.delete(known.keys().next().value as string);
}

/** Asks for one preview. `cancel` drops it if it hasn't started (its row left the screen). */
export function loadThumbnail(jobId: string, idx: number): { promise: Promise<string | null>; cancel: () => void } {
  let item!: Waiting;
  const promise = new Promise<string | null>((done) => {
    item = { jobId, idx, done, cancelled: false };
  });
  queue.push(item);
  pump();
  return { promise, cancel: () => (item.cancelled = true) };
}

function pump() {
  while (running < AT_ONCE && queue.length) {
    const item = queue.pop()!;
    if (item.cancelled) continue;
    const key = keyOf(item.jobId, item.idx);
    const had = known.get(key);
    if (had !== undefined) {
      item.done(had);
      continue;
    }
    running++;
    api
      .entryThumbnail(item.jobId, item.idx)
      .then((t) => {
        const url = t ? `data:${t.mime};base64,${t.base64}` : null;
        remember(key, url);
        item.done(url);
      })
      // Not remembered: a service that was away may answer next time.
      .catch(() => item.done(null))
      .finally(() => {
        running--;
        pump();
      });
  }
}
