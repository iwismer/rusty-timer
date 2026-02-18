import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { get } from "svelte/store";

// Mock fetch before importing modules that use it
const mockFetch = vi.fn();
vi.stubGlobal("fetch", mockFetch);

// Mock EventSource
class MockEventSource {
  static instances: MockEventSource[] = [];
  url: string;
  listeners: Record<string, ((e: MessageEvent) => void)[]> = {};
  onopen: (() => void) | null = null;
  onerror: (() => void) | null = null;
  readyState = 0;
  closed = false;

  constructor(url: string) {
    this.url = url;
    MockEventSource.instances.push(this);
    // Simulate async open
    setTimeout(() => {
      this.readyState = 1;
      if (this.onopen) this.onopen();
    }, 0);
  }

  addEventListener(type: string, listener: (e: MessageEvent) => void) {
    if (!this.listeners[type]) this.listeners[type] = [];
    this.listeners[type].push(listener);
  }

  removeEventListener() {}

  close() {
    this.closed = true;
    this.readyState = 2;
  }

  // Test helper: simulate a named SSE event
  emit(type: string, data: string) {
    const event = new MessageEvent(type, { data });
    for (const listener of this.listeners[type] ?? []) {
      listener(event);
    }
  }
}
vi.stubGlobal("EventSource", MockEventSource);

import { streamsStore, metricsStore, resetStores, setMetrics } from "./stores";
import type { StreamEntry } from "./api";

// Helper to make a mock Response
function makeResponse(body: unknown, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
    text: () => Promise.resolve(JSON.stringify(body)),
  };
}

describe("sse", () => {
  beforeEach(() => {
    resetStores();
    MockEventSource.instances = [];
    mockFetch.mockReset();
    // Default: getStreams returns empty
    mockFetch.mockResolvedValue(makeResponse({ streams: [] }));
  });

  afterEach(async () => {
    // Dynamic import so mocks are in place
    const { destroySSE } = await import("./sse");
    destroySSE();
  });

  it("opens EventSource to /api/v1/events", async () => {
    const { initSSE } = await import("./sse");
    initSSE();
    expect(MockEventSource.instances).toHaveLength(1);
    expect(MockEventSource.instances[0].url).toBe("/api/v1/events");
  });

  it("handles stream_created by adding to store", async () => {
    const { initSSE } = await import("./sse");
    initSSE();
    const es = MockEventSource.instances[0];

    const stream: StreamEntry = {
      stream_id: "s1",
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1",
      display_alias: null,
      online: true,
      stream_epoch: 1,
      created_at: "2026-01-01T00:00:00Z",
    };
    es.emit("stream_created", JSON.stringify(stream));

    expect(get(streamsStore)).toEqual([stream]);
  });

  it("handles stream_updated by patching store", async () => {
    const { initSSE } = await import("./sse");

    // Pre-populate store
    const { addOrUpdateStream } = await import("./stores");
    addOrUpdateStream({
      stream_id: "s1",
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1",
      display_alias: null,
      online: true,
      stream_epoch: 1,
      created_at: "2026-01-01T00:00:00Z",
    });

    initSSE();
    const es = MockEventSource.instances[0];
    es.emit(
      "stream_updated",
      JSON.stringify({ stream_id: "s1", online: false }),
    );

    expect(get(streamsStore)[0].online).toBe(false);
  });

  it("handles metrics_updated by setting in store", async () => {
    const { initSSE } = await import("./sse");
    initSSE();
    const es = MockEventSource.instances[0];
    es.emit(
      "metrics_updated",
      JSON.stringify({
        stream_id: "s1",
        raw_count: 42,
        dedup_count: 40,
        retransmit_count: 2,
        lag_ms: 100,
      }),
    );

    const m = get(metricsStore);
    expect(m.s1.raw_count).toBe(42);
    expect(m.s1.lag).toBe(100);
  });

  it("resync does not clear existing metrics", async () => {
    const { initSSE } = await import("./sse");
    setMetrics("s1", {
      raw_count: 7,
      dedup_count: 6,
      retransmit_count: 1,
      lag: 50,
      backlog: 0,
    });

    initSSE();
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(get(metricsStore).s1.raw_count).toBe(7);
  });
});
