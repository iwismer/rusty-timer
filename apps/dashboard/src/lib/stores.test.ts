import { describe, it, expect, beforeEach } from "vitest";
import { get } from "svelte/store";
import {
  streamsStore,
  metricsStore,
  addOrUpdateStream,
  patchStream,
  setMetrics,
  resetStores,
  replaceStreams,
} from "./stores";
import type { StreamEntry, StreamMetrics } from "./api";

const STREAM_A: StreamEntry = {
  stream_id: "aaa",
  forwarder_id: "fwd-1",
  reader_ip: "10.0.0.1",
  display_alias: null,
  online: true,
  stream_epoch: 1,
  created_at: "2026-01-01T00:00:00Z",
};

const METRICS_A: StreamMetrics = {
  raw_count: 10,
  dedup_count: 8,
  retransmit_count: 2,
  lag: 500,
  backlog: 0,
};

describe("stores", () => {
  beforeEach(() => {
    resetStores();
  });

  it("addOrUpdateStream adds a new stream", () => {
    addOrUpdateStream(STREAM_A);
    expect(get(streamsStore)).toEqual([STREAM_A]);
  });

  it("addOrUpdateStream updates existing stream by stream_id", () => {
    addOrUpdateStream(STREAM_A);
    addOrUpdateStream({ ...STREAM_A, online: false });
    const streams = get(streamsStore);
    expect(streams).toHaveLength(1);
    expect(streams[0].online).toBe(false);
  });

  it("patchStream merges partial fields", () => {
    addOrUpdateStream(STREAM_A);
    patchStream("aaa", { online: false, display_alias: "My Reader" });
    const streams = get(streamsStore);
    expect(streams[0].online).toBe(false);
    expect(streams[0].display_alias).toBe("My Reader");
    expect(streams[0].forwarder_id).toBe("fwd-1"); // unchanged
  });

  it("patchStream is a no-op for unknown stream_id", () => {
    addOrUpdateStream(STREAM_A);
    patchStream("unknown", { online: false });
    expect(get(streamsStore)).toEqual([STREAM_A]);
  });

  it("setMetrics sets metrics for a stream", () => {
    setMetrics("aaa", METRICS_A);
    expect(get(metricsStore)).toEqual({ aaa: METRICS_A });
  });

  it("setMetrics overwrites previous metrics", () => {
    setMetrics("aaa", METRICS_A);
    setMetrics("aaa", { ...METRICS_A, raw_count: 20 });
    expect(get(metricsStore).aaa.raw_count).toBe(20);
  });

  it("resetStores clears both stores", () => {
    addOrUpdateStream(STREAM_A);
    setMetrics("aaa", METRICS_A);
    resetStores();
    expect(get(streamsStore)).toEqual([]);
    expect(get(metricsStore)).toEqual({});
  });

  it("replaceStreams updates stream list without clearing metrics", () => {
    addOrUpdateStream(STREAM_A);
    setMetrics("aaa", METRICS_A);

    replaceStreams([
      {
        ...STREAM_A,
        stream_id: "bbb",
        forwarder_id: "fwd-2",
        reader_ip: "10.0.0.2",
      },
    ]);

    expect(get(streamsStore)).toEqual([
      {
        ...STREAM_A,
        stream_id: "bbb",
        forwarder_id: "fwd-2",
        reader_ip: "10.0.0.2",
      },
    ]);
    expect(get(metricsStore)).toEqual({ aaa: METRICS_A });
  });
});
