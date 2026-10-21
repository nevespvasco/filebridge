// The IPC requests a screen sends, written the way they travel over the socket
// (`{"type": …}` lines, crates/ipc). Shown in the Details panels; never contains secrets,
// because the requests don't carry any (ADR 0004).

export function request(type: string, fields: Record<string, unknown> = {}): string {
  return JSON.stringify({ type, ...fields });
}
