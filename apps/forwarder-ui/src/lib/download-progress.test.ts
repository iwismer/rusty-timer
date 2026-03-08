import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

class MockEventSource {
  static lastInstance: MockEventSource | null = null;

  onmessage: ((msg: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;

  constructor(public readonly url: string) {
    MockEventSource.lastInstance = this;
  }

  close() {
    this.closed = true;
  }
}

describe("subscribeDownloadProgress", () => {
  beforeEach(() => {
    vi.resetModules();
    MockEventSource.lastInstance = null;
    vi.stubGlobal(
      "EventSource",
      MockEventSource as unknown as typeof EventSource,
    );
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("connects to the correct SSE endpoint", async () => {
    const { subscribeDownloadProgress } = await import("./download-progress");
    subscribeDownloadProgress("192.168.1.10:7000", vi.fn());

    expect(MockEventSource.lastInstance!.url).toBe(
      "/api/v1/readers/192.168.1.10:7000/download-reads/progress",
    );
  });

  it("dispatches parsed events to onEvent callback", async () => {
    const { subscribeDownloadProgress } = await import("./download-progress");
    const onEvent = vi.fn();
    subscribeDownloadProgress("192.168.1.10:7000", onEvent);

    const es = MockEventSource.lastInstance!;
    es.onmessage!({
      data: JSON.stringify({
        state: "downloading",
        progress: 50,
        total: 100,
        reads_received: 25,
      }),
    });

    expect(onEvent).toHaveBeenCalledWith({
      state: "downloading",
      progress: 50,
      total: 100,
      reads_received: 25,
    });
  });

  it("auto-closes on complete state", async () => {
    const { subscribeDownloadProgress } = await import("./download-progress");
    const onEvent = vi.fn();
    subscribeDownloadProgress("192.168.1.10:7000", onEvent);

    const es = MockEventSource.lastInstance!;
    es.onmessage!({
      data: JSON.stringify({ state: "complete", reads_received: 42 }),
    });

    expect(onEvent).toHaveBeenCalled();
    expect(es.closed).toBe(true);
  });

  it("auto-closes on error state", async () => {
    const { subscribeDownloadProgress } = await import("./download-progress");
    const onEvent = vi.fn();
    subscribeDownloadProgress("192.168.1.10:7000", onEvent);

    const es = MockEventSource.lastInstance!;
    es.onmessage!({
      data: JSON.stringify({ state: "error", message: "connection lost" }),
    });

    expect(onEvent).toHaveBeenCalled();
    expect(es.closed).toBe(true);
  });

  it("does not close on downloading state", async () => {
    const { subscribeDownloadProgress } = await import("./download-progress");
    const onEvent = vi.fn();
    subscribeDownloadProgress("192.168.1.10:7000", onEvent);

    const es = MockEventSource.lastInstance!;
    es.onmessage!({
      data: JSON.stringify({
        state: "downloading",
        progress: 10,
        total: 100,
        reads_received: 5,
      }),
    });

    expect(es.closed).toBe(false);
  });

  it("calls onError and closes on EventSource error", async () => {
    const { subscribeDownloadProgress } = await import("./download-progress");
    const onEvent = vi.fn();
    const onError = vi.fn();
    subscribeDownloadProgress("192.168.1.10:7000", onEvent, onError);

    const es = MockEventSource.lastInstance!;
    es.onerror!();

    expect(onError).toHaveBeenCalled();
    expect(es.closed).toBe(true);
  });

  it("handles malformed JSON without crashing", async () => {
    const { subscribeDownloadProgress } = await import("./download-progress");
    const onEvent = vi.fn();
    const onError = vi.fn();
    const consoleSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    subscribeDownloadProgress("192.168.1.10:7000", onEvent, onError);

    const es = MockEventSource.lastInstance!;
    es.onmessage!({ data: "not valid json{{{" });

    expect(onEvent).not.toHaveBeenCalled();
    expect(consoleSpy).toHaveBeenCalled();
    expect(onError).toHaveBeenCalled();
    expect(es.closed).toBe(true);
    consoleSpy.mockRestore();
  });

  it("close handle closes the EventSource", async () => {
    const { subscribeDownloadProgress } = await import("./download-progress");
    const handle = subscribeDownloadProgress("192.168.1.10:7000", vi.fn());

    const es = MockEventSource.lastInstance!;
    expect(es.closed).toBe(false);

    handle.close();
    expect(es.closed).toBe(true);
  });
});
