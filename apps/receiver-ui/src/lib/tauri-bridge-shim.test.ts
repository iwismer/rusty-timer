import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { invoke, listen } from "./tauri-bridge-shim";

// ---------------------------------------------------------------------------
// Fake EventSource that records constructor URL and lets tests drive events.
// ---------------------------------------------------------------------------
type Listener = (ev: MessageEvent) => void;

class FakeEventSource {
  static instances: FakeEventSource[] = [];
  url: string;
  closed = false;
  closeCalls = 0;
  private listeners = new Map<string, Set<Listener>>();

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, listener: Listener): void {
    let set = this.listeners.get(type);
    if (!set) {
      set = new Set();
      this.listeners.set(type, set);
    }
    set.add(listener);
  }

  removeEventListener(type: string, listener: Listener): void {
    this.listeners.get(type)?.delete(listener);
  }

  close(): void {
    this.closed = true;
    this.closeCalls += 1;
  }

  listenerCount(type: string): number {
    return this.listeners.get(type)?.size ?? 0;
  }

  // Test helper: dispatch a named SSE event with raw string data.
  emit(type: string, data: string): void {
    const ev = { data } as MessageEvent;
    for (const listener of this.listeners.get(type) ?? []) {
      listener(ev);
    }
  }
}

const realFetch = globalThis.fetch;
const realEventSource = (globalThis as { EventSource?: unknown }).EventSource;

beforeEach(() => {
  FakeEventSource.instances = [];
  (globalThis as { EventSource?: unknown }).EventSource = FakeEventSource;
});

afterEach(() => {
  globalThis.fetch = realFetch;
  (globalThis as { EventSource?: unknown }).EventSource = realEventSource;
  vi.restoreAllMocks();
});

describe("tauri-bridge-shim invoke", () => {
  it("POSTs JSON to /bridge/invoke/:cmd with args and returns parsed JSON", async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ receiver_id: "recv-1" }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
    globalThis.fetch = fetchMock as typeof fetch;

    const result = await invoke<{ receiver_id: string }>("get_status", {
      foo: "bar",
    });

    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/bridge/invoke/get_status");
    expect(init.method).toBe("POST");
    expect(init.headers["Content-Type"]).toBe("application/json");
    expect(JSON.parse(init.body)).toEqual({ foo: "bar" });
    expect(result).toEqual({ receiver_id: "recv-1" });
  });

  it("sends an empty object body when no args are provided", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response("null", { status: 200 }));
    globalThis.fetch = fetchMock as typeof fetch;

    await invoke("get_status");

    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/bridge/invoke/get_status");
    expect(JSON.parse(init.body)).toEqual({});
  });

  it("encodes the command name in the URL", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response("{}", { status: 200 }));
    globalThis.fetch = fetchMock as typeof fetch;

    await invoke("weird/cmd name");

    expect(fetchMock.mock.calls[0][0]).toBe(
      "/bridge/invoke/weird%2Fcmd%20name",
    );
  });

  it("returns undefined for a 204 / empty response", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response(null, { status: 204 }));
    globalThis.fetch = fetchMock as typeof fetch;

    const result = await invoke("admin_clear_data");
    expect(result).toBeUndefined();
  });

  it("rejects non-2xx responses with status and body text", async () => {
    // Fresh Response per call: a Response body can only be consumed once.
    const fetchMock = vi.fn().mockImplementation(
      () =>
        new Response("unknown command", {
          status: 404,
          statusText: "Not Found",
        }),
    );
    globalThis.fetch = fetchMock as typeof fetch;

    await expect(invoke("does_not_exist")).rejects.toThrow(/404/);
    await expect(invoke("does_not_exist")).rejects.toThrow(/unknown command/);
  });

  it("does not leak request args in the rejection message", async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValue(new Response("boom", { status: 500 }));
    globalThis.fetch = fetchMock as typeof fetch;

    await expect(
      invoke("put_profile", { body: { token: "super-secret-token" } }),
    ).rejects.toThrow(
      expect.objectContaining({
        message: expect.not.stringContaining("super-secret-token"),
      }),
    );
  });
});

describe("tauri-bridge-shim listen", () => {
  it("opens an EventSource to /bridge/events and maps named events to the callback", async () => {
    const handler = vi.fn();
    const unlisten = await listen<{ value: number }>("status_changed", handler);

    expect(FakeEventSource.instances).toHaveLength(1);
    const source = FakeEventSource.instances[0];
    expect(source.url).toBe("/bridge/events");

    source.emit("status_changed", JSON.stringify({ value: 42 }));

    expect(handler).toHaveBeenCalledTimes(1);
    expect(handler).toHaveBeenCalledWith({
      event: "status_changed",
      payload: { value: 42 },
    });

    // Events with a different name must not trigger the handler.
    source.emit("other_event", JSON.stringify({ value: 7 }));
    expect(handler).toHaveBeenCalledTimes(1);

    await unlisten();
    expect(source.closed).toBe(true);

    // After unlisten the handler no longer receives events.
    source.emit("status_changed", JSON.stringify({ value: 99 }));
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it("multiplexes multiple listeners over one EventSource and tears it down after the last unlisten", async () => {
    const statusHandler = vi.fn();
    const resyncHandler = vi.fn();

    const unlistenStatus = await listen<{ value: number }>(
      "status_changed",
      statusHandler,
    );
    const unlistenResync = await listen<{ complete: boolean }>(
      "resync_progress",
      resyncHandler,
    );

    expect(FakeEventSource.instances).toHaveLength(1);
    const source = FakeEventSource.instances[0];
    expect(source.listenerCount("status_changed")).toBe(1);
    expect(source.listenerCount("resync_progress")).toBe(1);

    source.emit("status_changed", JSON.stringify({ value: 42 }));
    source.emit("resync_progress", JSON.stringify({ complete: true }));

    expect(statusHandler).toHaveBeenCalledTimes(1);
    expect(statusHandler).toHaveBeenCalledWith({
      event: "status_changed",
      payload: { value: 42 },
    });
    expect(resyncHandler).toHaveBeenCalledTimes(1);
    expect(resyncHandler).toHaveBeenCalledWith({
      event: "resync_progress",
      payload: { complete: true },
    });

    unlistenStatus();
    expect(source.listenerCount("status_changed")).toBe(0);
    expect(source.listenerCount("resync_progress")).toBe(1);
    expect(source.closed).toBe(false);
    expect(source.closeCalls).toBe(0);

    source.emit("status_changed", JSON.stringify({ value: 99 }));
    source.emit("resync_progress", JSON.stringify({ complete: false }));

    expect(statusHandler).toHaveBeenCalledTimes(1);
    expect(resyncHandler).toHaveBeenCalledTimes(2);

    unlistenStatus();
    expect(source.closeCalls).toBe(0);

    unlistenResync();
    expect(source.listenerCount("resync_progress")).toBe(0);
    expect(source.closed).toBe(true);
    expect(source.closeCalls).toBe(1);

    unlistenResync();
    expect(source.closeCalls).toBe(1);
  });

  it("ignores events with non-JSON data without throwing", async () => {
    const handler = vi.fn();
    const unlisten = await listen("resync", handler);
    const source = FakeEventSource.instances[0];

    expect(() => source.emit("resync", "not json")).not.toThrow();
    expect(handler).not.toHaveBeenCalled();

    unlisten();
  });
});
