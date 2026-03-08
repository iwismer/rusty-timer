import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

class MockEventSource {
  static lastInstance: MockEventSource | null = null;
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSED = 2;

  readonly CONNECTING = 0;
  readonly OPEN = 1;
  readonly CLOSED = 2;

  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  readyState: number = MockEventSource.CONNECTING;
  private listeners = new Map<string, Array<(event: { data: string }) => void>>();

  constructor(public readonly url: string) {
    MockEventSource.lastInstance = this;
  }

  addEventListener(type: string, listener: (event: { data: string }) => void) {
    const current = this.listeners.get(type) ?? [];
    current.push(listener);
    this.listeners.set(type, current);
  }

  close() {
    this.readyState = MockEventSource.CLOSED;
  }

  emit(type: string, payload: unknown) {
    const event = { data: JSON.stringify(payload) };
    for (const listener of this.listeners.get(type) ?? []) {
      listener(event);
    }
  }
}

describe("createSSE", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.useFakeTimers();
    MockEventSource.lastInstance = null;
    vi.stubGlobal("EventSource", MockEventSource as unknown as typeof EventSource);
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("dispatches named events to handlers", async () => {
    const { createSSE } = await import("./sse");
    const handler = vi.fn();
    createSSE("/api/v1/events", { my_event: handler });

    MockEventSource.lastInstance!.emit("my_event", { key: "value" });
    expect(handler).toHaveBeenCalledWith({ key: "value" });
  });

  it("calls onConnection(true) on open", async () => {
    const { createSSE } = await import("./sse");
    const onConnection = vi.fn();
    createSSE("/api/v1/events", {}, onConnection);

    const es = MockEventSource.lastInstance!;
    es.readyState = MockEventSource.OPEN;
    es.onopen!();
    expect(onConnection).toHaveBeenCalledWith(true);
  });

  it("signals disconnect immediately when readyState is CLOSED", async () => {
    const { createSSE } = await import("./sse");
    const onConnection = vi.fn();
    createSSE("/api/v1/events", {}, onConnection);

    const es = MockEventSource.lastInstance!;
    es.readyState = MockEventSource.CLOSED;
    es.onerror!();
    expect(onConnection).toHaveBeenCalledWith(false);
  });

  it("does not signal disconnect immediately when readyState is CONNECTING", async () => {
    const { createSSE } = await import("./sse");
    const onConnection = vi.fn();
    createSSE("/api/v1/events", {}, onConnection);

    const es = MockEventSource.lastInstance!;
    es.readyState = MockEventSource.CONNECTING;
    es.onerror!();
    expect(onConnection).not.toHaveBeenCalled();
  });

  it("signals disconnect after 10s fallback when readyState stays CONNECTING", async () => {
    const { createSSE } = await import("./sse");
    const onConnection = vi.fn();
    createSSE("/api/v1/events", {}, onConnection);

    const es = MockEventSource.lastInstance!;
    es.readyState = MockEventSource.CONNECTING;
    es.onerror!();
    expect(onConnection).not.toHaveBeenCalled();

    vi.advanceTimersByTime(10_000);
    expect(onConnection).toHaveBeenCalledWith(false);
  });

  it("cancels fallback timer when onopen fires before 10s", async () => {
    const { createSSE } = await import("./sse");
    const onConnection = vi.fn();
    createSSE("/api/v1/events", {}, onConnection);

    const es = MockEventSource.lastInstance!;
    es.readyState = MockEventSource.CONNECTING;
    es.onerror!();

    vi.advanceTimersByTime(2_000);
    es.readyState = MockEventSource.OPEN;
    es.onopen!();
    expect(onConnection).toHaveBeenCalledWith(true);

    vi.advanceTimersByTime(10_000);
    expect(onConnection).toHaveBeenCalledTimes(1);
    expect(onConnection).toHaveBeenCalledWith(true);
  });

  it("does not start multiple fallback timers on repeated errors", async () => {
    const { createSSE } = await import("./sse");
    const onConnection = vi.fn();
    createSSE("/api/v1/events", {}, onConnection);

    const es = MockEventSource.lastInstance!;
    es.readyState = MockEventSource.CONNECTING;
    es.onerror!();
    vi.advanceTimersByTime(3_000);
    es.onerror!();

    vi.advanceTimersByTime(7_000);
    expect(onConnection).toHaveBeenCalledTimes(1);
    expect(onConnection).toHaveBeenCalledWith(false);
  });

  it("does not crash on malformed JSON events", async () => {
    const { createSSE } = await import("./sse");
    const handler = vi.fn();
    const consoleSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    createSSE("/api/v1/events", { my_event: handler });

    const es = MockEventSource.lastInstance!;
    // Emit raw malformed JSON — not via .emit() which stringifies
    const listeners = (es as any).listeners.get("my_event") ?? [];
    for (const l of listeners) {
      l({ data: "not valid json{{{" });
    }

    expect(handler).not.toHaveBeenCalled();
    expect(consoleSpy).toHaveBeenCalled();
    consoleSpy.mockRestore();
  });

  it("logs handler errors differently from parse errors", async () => {
    const { createSSE } = await import("./sse");
    const handler = vi.fn().mockImplementation(() => {
      throw new TypeError("Cannot read properties of undefined");
    });
    const consoleSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    createSSE("/api/v1/events", { my_event: handler });

    const es = MockEventSource.lastInstance!;
    es.emit("my_event", { key: "value" });

    expect(handler).toHaveBeenCalledWith({ key: "value" });
    expect(consoleSpy).toHaveBeenCalledTimes(1);
    expect(consoleSpy.mock.calls[0][0]).toBe('SSE handler error for "my_event":');
    consoleSpy.mockRestore();
  });

  it("cleans up fallback timer on destroy", async () => {
    const { createSSE } = await import("./sse");
    const onConnection = vi.fn();
    const handle = createSSE("/api/v1/events", {}, onConnection);

    const es = MockEventSource.lastInstance!;
    es.readyState = MockEventSource.CONNECTING;
    es.onerror!();

    handle.destroy();

    vi.advanceTimersByTime(10_000);
    expect(onConnection).not.toHaveBeenCalled();
  });
});
