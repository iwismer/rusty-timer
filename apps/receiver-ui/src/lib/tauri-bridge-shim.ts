// Tauri API shim for the headless control bridge (T5.3).
//
// This module mirrors the slice of `@tauri-apps/api/core` (`invoke`) and
// `@tauri-apps/api/event` (`listen`, `UnlistenFn`) that the receiver UI uses.
// It is wired in **only** when Vite builds with `--mode e2e`, via an alias in
// `vite.config.ts`. Production builds keep using the real Tauri package, so
// this file must never be imported by application code directly.
//
// Transport contract (served by the receiver headless host, T5.1):
//   * `invoke(cmd, args)` -> `POST /bridge/invoke/:cmd` with a JSON body.
//   * `listen(event, cb)` -> Server-Sent Events stream at `/bridge/events`.
//
// `fetch` and `EventSource` are read from the global scope at call time so
// tests can substitute fakes without any production wiring.

/** Matches Tauri's `UnlistenFn`: a zero-arg function that detaches a listener. */
export type UnlistenFn = () => void;

/** Matches the shape of Tauri's event callback argument. */
export interface ShimEvent<T> {
  event: string;
  payload: T;
}

/**
 * Dispatch a canonical control command to the headless bridge.
 *
 * Mirrors `@tauri-apps/api/core`'s `invoke`: posts `args ?? {}` as JSON to
 * `/bridge/invoke/:cmd`. A 2xx JSON body is parsed and returned; a 204 or
 * empty body resolves to `undefined`. Non-2xx responses reject with an error
 * carrying the status and response text — never the request arguments, which
 * may contain secrets.
 */
export async function invoke<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const response = await fetch(`/bridge/invoke/${encodeURIComponent(cmd)}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(args ?? {}),
  });

  if (!response.ok) {
    let detail = "";
    try {
      detail = (await response.text()).trim();
    } catch {
      // Body already consumed or unreadable; status alone is still useful.
    }
    const suffix = detail ? `: ${detail}` : "";
    throw new Error(
      `bridge invoke "${cmd}" failed (${response.status} ${response.statusText})${suffix}`,
    );
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  if (text === "") {
    return undefined as T;
  }
  return JSON.parse(text) as T;
}

/**
 * Subscribe to a named bridge event.
 *
 * Mirrors `@tauri-apps/api/event`'s `listen`: opens an `EventSource` to
 * `/bridge/events`, forwards events named `event` (with their JSON-parsed
 * data) to `handler` as `{ event, payload }`, and resolves with an unlisten
 * function that detaches the handler. All listeners share one SSE stream, which
 * closes after the last listener is removed.
 */
let sharedEventSource: EventSource | undefined;
let activeListenerCount = 0;

export function listen<T = unknown>(
  event: string,
  handler: (event: ShimEvent<T>) => void,
): Promise<UnlistenFn> {
  sharedEventSource ??= new EventSource("/bridge/events");
  const source = sharedEventSource;
  activeListenerCount += 1;

  const onMessage = (ev: MessageEvent) => {
    let payload: T;
    try {
      payload = JSON.parse(ev.data) as T;
    } catch {
      // Malformed SSE data: drop it rather than crashing the listener.
      return;
    }
    handler({ event, payload });
  };

  source.addEventListener(event, onMessage as EventListener);

  let active = true;
  const unlisten: UnlistenFn = () => {
    if (!active) {
      return;
    }
    active = false;

    source.removeEventListener(event, onMessage as EventListener);
    activeListenerCount -= 1;

    if (activeListenerCount === 0 && sharedEventSource === source) {
      sharedEventSource = undefined;
      source.close();
    }
  };

  return Promise.resolve(unlisten);
}

// ---------------------------------------------------------------------------
// Minimal `@tauri-apps/api/core` compatibility stubs.
//
// Aliasing `@tauri-apps/api/core` to this shim also redirects the bundled
// `@tauri-apps/plugin-*` packages, which import `Resource` and `Channel` from
// core. The bridge has no IPC resource/channel transport, so these are inert
// placeholders that exist only so the e2e bundle builds and loads. Any plugin
// command that reaches them resolves through `invoke` -> the HTTP bridge.
// ---------------------------------------------------------------------------

/** Inert stand-in for Tauri's `Resource` (no IPC handle in bridge mode). */
export class Resource {
  readonly rid: number;

  constructor(rid = 0) {
    this.rid = rid;
  }

  async close(): Promise<void> {
    // No backing IPC resource to release in bridge mode.
  }
}

/** Inert stand-in for Tauri's `Channel` (no IPC callback channel). */
export class Channel<T = unknown> {
  id = 0;
  onmessage: (message: T) => void;

  constructor(onmessage?: (message: T) => void) {
    this.onmessage = onmessage ?? (() => {});
  }

  toJSON(): string {
    return `__CHANNEL__:${this.id}`;
  }
}
