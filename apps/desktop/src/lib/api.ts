// Thin helpers over the generated bindings. Every call to the service goes through `commands`.
import { commands, type ErrorBody } from "@/bindings";

export { commands, events } from "@/bindings";

/** A service error, with its stable `code` (see crates/ipc/README.md). */
export class ServiceError extends Error {
  readonly code: string;
  constructor(body: ErrorBody) {
    super(body.message);
    this.code = body.code;
  }
}

type Result<T> = { status: "ok"; data: T } | { status: "error"; error: ErrorBody };

/** Turns a command's result object into a value or a thrown `ServiceError`. */
export async function unwrap<T>(call: Promise<Result<T>>): Promise<T> {
  const r = await call;
  if (r.status === "error") throw new ServiceError(r.error);
  return r.data;
}

/** Shorthand: `api.listJobs(50, 0)` resolves to the data or throws. */
type Commands = typeof commands;
type Unwrapped = {
  [K in keyof Commands]: Commands[K] extends (...args: infer A) => Promise<Result<infer T>>
    ? (...args: A) => Promise<T>
    : Commands[K];
};

export const api = new Proxy(commands, {
  get(target, key: string) {
    const fn = (target as Record<string, (...a: unknown[]) => unknown>)[key];
    return (...args: unknown[]) => {
      const out = fn(...args);
      return out instanceof Promise
        ? out.then((r) =>
            r && typeof r === "object" && "status" in r
              ? unwrap(Promise.resolve(r as Result<unknown>))
              : r,
          )
        : out;
    };
  },
}) as unknown as Unwrapped;

export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return JSON.stringify(e);
}
