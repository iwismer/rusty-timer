import { describe, it, expect, vi, beforeEach } from "vitest";

class MockEventSource {
  static lastInstance: MockEventSource | null = null;
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  private listeners = new Map<string, Array<(event: { data: string }) => void>>();

  constructor(public readonly url: string) {
    MockEventSource.lastInstance = this;
  }

  addEventListener(type: string, listener: (event: { data: string }) => void) {
    const current = this.listeners.get(type) ?? [];
    current.push(listener);
    this.listeners.set(type, current);
  }

  close() {}

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
    MockEventSource.lastInstance = null;
    vi.stubGlobal("EventSource", MockEventSource as unknown as typeof EventSource);
  });

  it("dispatches named events to handlers", async () => {
    const { createSSE } = await import("./sse");
    const handler = vi.fn();
    const sse = createSSE("/api/v1/events", { my_event: handler });

    MockEventSource.lastInstance!.emit("my_event", { key: "value" });
    expect(handler).toHaveBeenCalledWith({ key: "value" });

    sse.destroy();
    vi.unstubAllGlobals();
  });

  it("calls onConnection(true) on open", async () => {
    const { createSSE } = await import("./sse");
    const onConnection = vi.fn();
    const sse = createSSE("/api/v1/events", {}, onConnection);

    MockEventSource.lastInstance!.onopen!();
    expect(onConnection).toHaveBeenCalledWith(true);

    sse.destroy();
    vi.unstubAllGlobals();
  });

  it("calls onConnection(false) on error", async () => {
    const { createSSE } = await import("./sse");
    const onConnection = vi.fn();
    const sse = createSSE("/api/v1/events", {}, onConnection);

    MockEventSource.lastInstance!.onerror!();
    expect(onConnection).toHaveBeenCalledWith(false);

    sse.destroy();
    vi.unstubAllGlobals();
  });
});
