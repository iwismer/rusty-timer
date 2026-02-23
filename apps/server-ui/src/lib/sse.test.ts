import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { get } from "svelte/store";

// Mock fetch before importing modules that use it
const mockFetch = vi.fn();
vi.stubGlobal("fetch", mockFetch);

// Mock EventSource
class MockEventSource {
  static instances: MockEventSource[] = [];
  static openDelayMs = 0;
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
      if (this.closed) return;
      this.readyState = 1;
      if (this.onopen) this.onopen();
    }, MockEventSource.openDelayMs);
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

import {
  streamsStore,
  metricsStore,
  resetStores,
  setMetrics,
  racesStore,
  forwarderRacesStore,
  logsStore,
} from "./stores";
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
    MockEventSource.openDelayMs = 0;
    mockFetch.mockReset();
    // Default: all resync endpoints return empty payloads
    mockFetch.mockImplementation((input: unknown) => {
      const url = String(input);
      if (url.includes("/api/v1/streams")) {
        return Promise.resolve(makeResponse({ streams: [] }));
      }
      if (url.includes("/api/v1/races")) {
        return Promise.resolve(makeResponse({ races: [] }));
      }
      if (url.includes("/api/v1/forwarder-races")) {
        return Promise.resolve(makeResponse({ assignments: [] }));
      }
      if (url.includes("/api/v1/logs")) {
        return Promise.resolve(makeResponse({ entries: [] }));
      }
      return Promise.reject(new Error(`unexpected fetch URL: ${url}`));
    });
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
      reader_ip: "10.0.0.1:10000",
      display_alias: null,
      forwarder_display_name: null,
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
      reader_ip: "10.0.0.1:10000",
      display_alias: null,
      forwarder_display_name: null,
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

  it("notifies stream_updated listeners and honors unsubscribe", async () => {
    const { initSSE, onStreamUpdated } = await import("./sse");
    const listener = vi.fn();
    const unsubscribe = onStreamUpdated(listener);

    initSSE();
    const es = MockEventSource.instances[0];
    es.emit(
      "stream_updated",
      JSON.stringify({ stream_id: "s1", stream_epoch: 3 }),
    );

    expect(listener).toHaveBeenCalledWith({ stream_id: "s1", stream_epoch: 3 });

    unsubscribe();
    es.emit(
      "stream_updated",
      JSON.stringify({ stream_id: "s1", stream_epoch: 4 }),
    );
    expect(listener).toHaveBeenCalledTimes(1);
  });

  it("handles stream_updated forwarder_display_name patch", async () => {
    const { initSSE } = await import("./sse");

    const { addOrUpdateStream } = await import("./stores");
    addOrUpdateStream({
      stream_id: "s1",
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1",
      display_alias: null,
      forwarder_display_name: "Start Line",
      online: true,
      stream_epoch: 1,
      created_at: "2026-01-01T00:00:00Z",
    });

    initSSE();
    const es = MockEventSource.instances[0];
    es.emit(
      "stream_updated",
      JSON.stringify({
        stream_id: "s1",
        forwarder_display_name: "Finish Line",
      }),
    );

    expect(get(streamsStore)[0].forwarder_display_name).toBe("Finish Line");
  });

  it("handles stream_updated forwarder_display_name clear patch", async () => {
    const { initSSE } = await import("./sse");

    const { addOrUpdateStream } = await import("./stores");
    addOrUpdateStream({
      stream_id: "s1",
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1",
      display_alias: null,
      forwarder_display_name: "Start Line",
      online: true,
      stream_epoch: 1,
      created_at: "2026-01-01T00:00:00Z",
    });

    initSSE();
    const es = MockEventSource.instances[0];
    es.emit(
      "stream_updated",
      JSON.stringify({
        stream_id: "s1",
        forwarder_display_name: null,
      }),
    );

    expect(get(streamsStore)[0].forwarder_display_name).toBeNull();
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
        epoch_raw_count: 10,
        epoch_dedup_count: 9,
        epoch_retransmit_count: 1,
        epoch_lag_ms: 50,
        epoch_last_received_at: "2026-02-18T12:00:00Z",
        unique_chips: 5,
        last_tag_id: null,
        last_reader_timestamp: null,
      }),
    );

    const m = get(metricsStore);
    expect(m.s1.raw_count).toBe(42);
    expect(m.s1.lag).toBe(100);
    expect(m.s1.epoch_raw_count).toBe(10);
    expect(m.s1.epoch_dedup_count).toBe(9);
    expect(m.s1.epoch_retransmit_count).toBe(1);
    expect(m.s1.epoch_lag).toBe(50);
    expect(m.s1.epoch_last_received_at).toBe("2026-02-18T12:00:00Z");
    expect(m.s1.unique_chips).toBe(5);
  });

  it("resync does not clear existing metrics", async () => {
    const { initSSE } = await import("./sse");
    setMetrics("s1", {
      raw_count: 7,
      dedup_count: 6,
      retransmit_count: 1,
      lag: 50,
      backlog: 0,
      epoch_raw_count: 3,
      epoch_dedup_count: 2,
      epoch_retransmit_count: 1,
      epoch_lag: 20,
      epoch_last_received_at: "2026-02-18T12:00:00Z",
      unique_chips: 2,
      last_tag_id: null,
      last_reader_timestamp: null,
    });

    initSSE();
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(get(metricsStore).s1.raw_count).toBe(7);
  });

  it("resync refreshes races and forwarder assignments", async () => {
    const { initSSE } = await import("./sse");
    const { setRaces, setForwarderRace } = await import("./stores");

    setRaces([
      {
        race_id: "11111111-1111-1111-1111-111111111111",
        name: "Stale Race",
        created_at: "2026-01-01T00:00:00Z",
        participant_count: 1,
        chip_count: 1,
      },
    ]);
    setForwarderRace("fwd-1", "11111111-1111-1111-1111-111111111111");

    initSSE();
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(get(racesStore)).toEqual([]);
    expect(get(forwarderRacesStore)).toEqual({});
  });

  it("resync still updates streams when race refresh fails", async () => {
    const { initSSE } = await import("./sse");

    mockFetch.mockImplementation((input: unknown) => {
      const url = String(input);
      if (url.includes("/api/v1/streams")) {
        return Promise.resolve(
          makeResponse({
            streams: [
              {
                stream_id: "s1",
                forwarder_id: "fwd-1",
                reader_ip: "10.0.0.1:10000",
                display_alias: null,
                forwarder_display_name: null,
                online: true,
                stream_epoch: 1,
                created_at: "2026-01-01T00:00:00Z",
              },
            ],
          }),
        );
      }
      if (url.includes("/api/v1/races")) {
        return Promise.resolve(makeResponse({ code: "boom" }, 500));
      }
      if (url.includes("/api/v1/forwarder-races")) {
        return Promise.resolve(makeResponse({ assignments: [] }));
      }
      if (url.includes("/api/v1/logs")) {
        return Promise.resolve(makeResponse({ entries: [] }));
      }
      return Promise.reject(new Error(`unexpected fetch URL: ${url}`));
    });

    initSSE();
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(get(streamsStore)).toHaveLength(1);
    expect(get(streamsStore)[0].stream_id).toBe("s1");
  });

  it("fetches streams eagerly before open, then once again on open", async () => {
    MockEventSource.openDelayMs = 30;
    mockFetch.mockImplementation((input: unknown) => {
      const url = String(input);
      if (url.includes("/api/v1/streams")) {
        return Promise.resolve(makeResponse({ streams: [] }));
      }
      if (url.includes("/api/v1/races")) {
        return Promise.resolve(makeResponse({ races: [] }));
      }
      if (url.includes("/api/v1/forwarder-races")) {
        return Promise.resolve(makeResponse({ assignments: [] }));
      }
      if (url.includes("/api/v1/logs")) {
        return Promise.resolve(makeResponse({ entries: [] }));
      }
      return Promise.reject(new Error(`unexpected fetch URL: ${url}`));
    });

    const { initSSE } = await import("./sse");
    initSSE();

    // Eager startup sync should run immediately (streams, races, forwarder assignments, and logs).
    expect(mockFetch).toHaveBeenCalledTimes(4);

    // onopen should trigger exactly one follow-up sync.
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(mockFetch).toHaveBeenCalledTimes(8);

    // No additional fetches without explicit resync triggers.
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(mockFetch).toHaveBeenCalledTimes(8);
  });

  it("updates streams even when logs fetch fails during resync", async () => {
    const { initSSE } = await import("./sse");
    const stream: StreamEntry = {
      stream_id: "s1",
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      display_alias: null,
      forwarder_display_name: null,
      online: true,
      stream_epoch: 1,
      created_at: "2026-01-01T00:00:00Z",
    };

    MockEventSource.openDelayMs = 30;
    mockFetch.mockReset();
    let logFetches = 0;
    mockFetch.mockImplementation((input: RequestInfo | URL) => {
      const url = String(input);
      if (url.includes("/api/v1/streams")) {
        return Promise.resolve(makeResponse({ streams: [stream] }));
      }
      if (url.includes("/api/v1/logs")) {
        logFetches += 1;
        if (logFetches === 1) {
          return Promise.reject(new Error("logs down"));
        }
        return Promise.resolve(makeResponse({ entries: [] }));
      }
      if (url.includes("/api/v1/races")) {
        return Promise.resolve(makeResponse({ races: [] }));
      }
      if (url.includes("/api/v1/forwarder-races")) {
        return Promise.resolve(makeResponse({ assignments: [] }));
      }
      return Promise.reject(new Error(`unexpected fetch URL: ${url}`));
    });

    initSSE();
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(get(streamsStore)).toEqual([stream]);
  });

  it("runs a follow-up resync when resync events arrive during an in-flight resync", async () => {
    const streamA: StreamEntry = {
      stream_id: "s1",
      forwarder_id: "fwd-1",
      reader_ip: "10.0.0.1:10000",
      display_alias: null,
      forwarder_display_name: null,
      online: false,
      stream_epoch: 1,
      created_at: "2026-01-01T00:00:00Z",
    };
    const streamB: StreamEntry = { ...streamA, online: true };

    let streamCalls = 0;
    let resolveFirstStream!: (value: ReturnType<typeof makeResponse>) => void;
    const firstStream = new Promise<ReturnType<typeof makeResponse>>(
      (resolve) => {
        resolveFirstStream = resolve;
      },
    );

    MockEventSource.openDelayMs = 100;
    mockFetch.mockReset();
    mockFetch.mockImplementation((input: unknown) => {
      const url = String(input);
      if (url.includes("/api/v1/streams")) {
        streamCalls += 1;
        if (streamCalls === 1) {
          return firstStream;
        }
        return Promise.resolve(makeResponse({ streams: [streamB] }));
      }
      if (url.includes("/api/v1/races")) {
        return Promise.resolve(makeResponse({ races: [] }));
      }
      if (url.includes("/api/v1/forwarder-races")) {
        return Promise.resolve(makeResponse({ assignments: [] }));
      }
      if (url.includes("/api/v1/logs")) {
        return Promise.resolve(makeResponse({ entries: [] }));
      }
      return Promise.reject(new Error(`unexpected fetch URL: ${url}`));
    });

    const { initSSE } = await import("./sse");
    initSSE();
    const es = MockEventSource.instances[0];

    await new Promise((resolve) => setTimeout(resolve, 0));
    es.emit("resync", "{}");

    expect(mockFetch).toHaveBeenCalledTimes(4);

    resolveFirstStream(makeResponse({ streams: [streamA] }));
    await new Promise((resolve) => setTimeout(resolve, 20));

    expect(mockFetch).toHaveBeenCalledTimes(8);
    expect(streamCalls).toBe(2);
    expect(get(streamsStore)[0].online).toBe(true);
  });

  it("keeps live log entries that arrive during an in-flight logs resync", async () => {
    const { initSSE } = await import("./sse");
    MockEventSource.openDelayMs = 200;

    let resolveLogs!: (value: ReturnType<typeof makeResponse>) => void;
    const delayedLogs = new Promise<ReturnType<typeof makeResponse>>(
      (resolve) => {
        resolveLogs = resolve;
      },
    );

    mockFetch.mockReset();
    mockFetch.mockImplementation((input: unknown) => {
      const url = String(input);
      if (url.includes("/api/v1/streams")) {
        return Promise.resolve(makeResponse({ streams: [] }));
      }
      if (url.includes("/api/v1/races")) {
        return Promise.resolve(makeResponse({ races: [] }));
      }
      if (url.includes("/api/v1/forwarder-races")) {
        return Promise.resolve(makeResponse({ assignments: [] }));
      }
      if (url.includes("/api/v1/logs")) {
        return delayedLogs;
      }
      return Promise.reject(new Error(`unexpected fetch URL: ${url}`));
    });

    initSSE();
    const es = MockEventSource.instances[0];

    await new Promise((resolve) => setTimeout(resolve, 0));
    es.emit(
      "log_entry",
      JSON.stringify({ entry: "15:00:00 [INFO] live entry" }),
    );
    resolveLogs(makeResponse({ entries: ["14:59:59 [INFO] snapshot entry"] }));
    await new Promise((resolve) => setTimeout(resolve, 10));

    expect(get(logsStore)).toEqual([
      "14:59:59 [INFO] snapshot entry",
      "15:00:00 [INFO] live entry",
    ]);
  });
});
