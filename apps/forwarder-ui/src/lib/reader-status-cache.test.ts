import { describe, expect, it } from "vitest";
import type { ForwarderStatus, ReaderInfo } from "./api";
import {
  applyReaderInfoUpdate,
  clearReaderInfoForIp,
  rebuildReaderCachesFromStatus,
} from "./reader-status-cache";

function makeInfo(overrides: Partial<ReaderInfo> = {}): ReaderInfo {
  return {
    banner: "IPICO V2",
    clock: { reader_clock: "2026-03-08 12:34:56", drift_ms: 0 },
    connect_failures: 0,
    ...overrides,
  };
}

function makeStatus(readers: ForwarderStatus["readers"]): ForwarderStatus {
  return {
    forwarder_id: "fwd-1",
    version: "0.1.0",
    ready: true,
    ready_reason: null,
    uplink_connected: true,
    restart_needed: false,
    readers,
  };
}

describe("rebuildReaderCachesFromStatus", () => {
  it("drops stale reader info and clock cache entries when snapshot omits reader_info", () => {
    const now = 1234567890;
    const result = rebuildReaderCachesFromStatus(
      makeStatus([
        {
          ip: "10.0.0.42",
          state: "disconnected",
          reads_session: 1,
          reads_total: 2,
          last_seen_secs: 3,
          local_port: 10042,
          reader_info: null,
        },
      ]),
      {
        readerInfoMap: { "10.0.0.42": makeInfo() },
        readerInfoReceivedAt: { "10.0.0.42": 111 },
        readerClockBaseTs: { "10.0.0.42": 222 },
        readerClockBaseLocal: { "10.0.0.42": 333 },
        lastSeenBase: {},
        lastSeenReceivedAt: {},
      },
      now,
    );

    expect(result.readerInfoMap).toEqual({});
    expect(result.readerInfoReceivedAt).toEqual({});
    expect(result.readerClockBaseTs).toEqual({});
    expect(result.readerClockBaseLocal).toEqual({});
    expect(result.lastSeenBase).toEqual({ "10.0.0.42": 3 });
    expect(result.lastSeenReceivedAt).toEqual({ "10.0.0.42": now });
  });

  it("rebuilds caches for readers that still include reader_info", () => {
    const now = 1234567890;
    const info = makeInfo({
      clock: { reader_clock: "2026-03-08 09:10:11", drift_ms: 12 },
    });

    const result = rebuildReaderCachesFromStatus(
      makeStatus([
        {
          ip: "10.0.0.99",
          state: "connected",
          reads_session: 5,
          reads_total: 6,
          last_seen_secs: 7,
          local_port: 10099,
          reader_info: info,
        },
      ]),
      {
        readerInfoMap: {},
        readerInfoReceivedAt: {},
        readerClockBaseTs: {},
        readerClockBaseLocal: {},
        lastSeenBase: {},
        lastSeenReceivedAt: {},
      },
      now,
    );

    expect(result.readerInfoMap).toEqual({ "10.0.0.99": info });
    expect(result.readerInfoReceivedAt).toEqual({ "10.0.0.99": now });
    expect(result.readerClockBaseTs["10.0.0.99"]).toBeGreaterThan(0);
    expect(result.readerClockBaseLocal).toEqual({ "10.0.0.99": now });
    expect(result.lastSeenBase).toEqual({ "10.0.0.99": 7 });
    expect(result.lastSeenReceivedAt).toEqual({ "10.0.0.99": now });
  });

  it("handles mixed connected and disconnected readers", () => {
    const now = 1234567890;
    const info = makeInfo();
    const result = rebuildReaderCachesFromStatus(
      makeStatus([
        {
          ip: "10.0.0.1",
          state: "connected",
          reads_session: 10,
          reads_total: 20,
          last_seen_secs: 5,
          local_port: 10001,
          reader_info: info,
        },
        {
          ip: "10.0.0.2",
          state: "disconnected",
          reads_session: 0,
          reads_total: 0,
          last_seen_secs: null,
          local_port: 10002,
          reader_info: null,
        },
      ]),
      {
        readerInfoMap: { "10.0.0.2": makeInfo({ banner: "stale" }) },
        readerInfoReceivedAt: { "10.0.0.2": 111 },
        readerClockBaseTs: { "10.0.0.2": 222 },
        readerClockBaseLocal: { "10.0.0.2": 333 },
        lastSeenBase: {},
        lastSeenReceivedAt: {},
      },
      now,
    );

    // Connected reader has info
    expect(result.readerInfoMap["10.0.0.1"]).toEqual(info);
    expect(result.readerInfoReceivedAt["10.0.0.1"]).toBe(now);
    // Disconnected reader's stale info is gone
    expect(result.readerInfoMap["10.0.0.2"]).toBeUndefined();
    expect(result.readerInfoReceivedAt["10.0.0.2"]).toBeUndefined();
    expect(result.readerClockBaseTs["10.0.0.2"]).toBeUndefined();
    // Both have lastSeenBase
    expect(result.lastSeenBase["10.0.0.1"]).toBe(5);
    expect(result.lastSeenBase["10.0.0.2"]).toBeNull();
  });
});

describe("applyReaderInfoUpdate", () => {
  it("ignores late reader info updates for disconnected readers", () => {
    const now = 1234567890;
    const previous = {
      readerInfoMap: {},
      readerInfoReceivedAt: {},
      readerClockBaseTs: {},
      readerClockBaseLocal: {},
      lastSeenBase: {},
      lastSeenReceivedAt: {},
    };

    const result = applyReaderInfoUpdate(
      makeStatus([
        {
          ip: "10.0.0.42",
          state: "disconnected",
          reads_session: 1,
          reads_total: 2,
          last_seen_secs: 3,
          local_port: 10042,
          reader_info: null,
        },
      ]),
      previous,
      {
        ip: "10.0.0.42",
        banner: "late-info",
        clock: { reader_clock: "2026-03-08 12:34:56", drift_ms: 0 },
        connect_failures: 0,
      },
      now,
    );

    expect(result).toEqual(previous);
  });

  it("applies update when status is null (pre-first-fetch)", () => {
    const now = 1234567890;
    const previous = {
      readerInfoMap: {},
      readerInfoReceivedAt: {},
      readerClockBaseTs: {},
      readerClockBaseLocal: {},
      lastSeenBase: {},
      lastSeenReceivedAt: {},
    };

    const result = applyReaderInfoUpdate(
      null,
      previous,
      {
        ip: "10.0.0.42",
        banner: "IPICO V2",
        clock: { reader_clock: "2026-03-08 12:34:56", drift_ms: 0 },
        connect_failures: 0,
      },
      now,
    );

    expect(result.readerInfoMap["10.0.0.42"]).toBeDefined();
    expect(result.readerInfoMap["10.0.0.42"].banner).toBe("IPICO V2");
    expect(result.readerInfoReceivedAt["10.0.0.42"]).toBe(now);
    expect(result.readerClockBaseTs["10.0.0.42"]).toBeGreaterThan(0);
    expect(result.readerClockBaseLocal["10.0.0.42"]).toBe(now);
  });

  it("applies update and populates clock for connected reader", () => {
    const now = 1234567890;
    const previous = {
      readerInfoMap: { "10.0.0.42": makeInfo({ banner: "old" }) },
      readerInfoReceivedAt: { "10.0.0.42": 100 },
      readerClockBaseTs: {},
      readerClockBaseLocal: {},
      lastSeenBase: {},
      lastSeenReceivedAt: {},
    };

    const result = applyReaderInfoUpdate(
      makeStatus([
        {
          ip: "10.0.0.42",
          state: "connected",
          reads_session: 0,
          reads_total: 0,
          last_seen_secs: 0,
          local_port: 10042,
          reader_info: null,
        },
      ]),
      previous,
      {
        ip: "10.0.0.42",
        banner: "updated",
        clock: { reader_clock: "2026-03-08 14:00:00", drift_ms: 5 },
        connect_failures: 0,
      },
      now,
    );

    expect(result.readerInfoMap["10.0.0.42"].banner).toBe("updated");
    expect(result.readerInfoReceivedAt["10.0.0.42"]).toBe(now);
    expect(result.readerClockBaseTs["10.0.0.42"]).toBeGreaterThan(0);
    expect(result.readerClockBaseLocal["10.0.0.42"]).toBe(now);
  });

  it("preserves existing fields when partial update arrives", () => {
    const now = 2000;
    const previous = {
      readerInfoMap: {
        "10.0.0.42": makeInfo({
          banner: "IPICO V2",
          recording: false,
        }),
      },
      readerInfoReceivedAt: { "10.0.0.42": 1000 },
      readerClockBaseTs: {},
      readerClockBaseLocal: {},
      lastSeenBase: {},
      lastSeenReceivedAt: {},
    };

    const result = applyReaderInfoUpdate(
      makeStatus([
        {
          ip: "10.0.0.42",
          state: "connected",
          reads_session: 0,
          reads_total: 0,
          last_seen_secs: 0,
          local_port: 10042,
          reader_info: null,
        },
      ]),
      previous,
      {
        ip: "10.0.0.42",
        recording: true,
        connect_failures: 0,
      },
      now,
    );

    // recording updated
    expect(result.readerInfoMap["10.0.0.42"].recording).toBe(true);
    // banner preserved from previous
    expect(result.readerInfoMap["10.0.0.42"].banner).toBe("IPICO V2");
    // receivedAt updated
    expect(result.readerInfoReceivedAt["10.0.0.42"]).toBe(now);
  });
});

describe("clearReaderInfoForIp", () => {
  it("removes all reader-info caches for the given IP", () => {
    const previous = {
      readerInfoMap: { "10.0.0.42": makeInfo() },
      readerInfoReceivedAt: { "10.0.0.42": 111 },
      readerClockBaseTs: { "10.0.0.42": 222 },
      readerClockBaseLocal: { "10.0.0.42": 333 },
      lastSeenBase: { "10.0.0.42": 5 },
      lastSeenReceivedAt: { "10.0.0.42": 444 },
    };
    const result = clearReaderInfoForIp(previous, "10.0.0.42");

    expect(result.readerInfoMap).toEqual({});
    expect(result.readerInfoReceivedAt).toEqual({});
    expect(result.readerClockBaseTs).toEqual({});
    expect(result.readerClockBaseLocal).toEqual({});
    expect(result.lastSeenBase).toEqual({ "10.0.0.42": 5 });
    expect(result.lastSeenReceivedAt).toEqual({ "10.0.0.42": 444 });
  });

  it("preserves other readers' caches", () => {
    const previous = {
      readerInfoMap: {
        "10.0.0.42": makeInfo(),
        "10.0.0.99": makeInfo({ banner: "other" }),
      },
      readerInfoReceivedAt: { "10.0.0.42": 111, "10.0.0.99": 222 },
      readerClockBaseTs: { "10.0.0.42": 333, "10.0.0.99": 444 },
      readerClockBaseLocal: { "10.0.0.42": 555, "10.0.0.99": 666 },
      lastSeenBase: {},
      lastSeenReceivedAt: {},
    };
    const result = clearReaderInfoForIp(previous, "10.0.0.42");

    expect(result.readerInfoMap).toEqual({
      "10.0.0.99": makeInfo({ banner: "other" }),
    });
    expect(result.readerInfoReceivedAt).toEqual({ "10.0.0.99": 222 });
    expect(result.readerClockBaseTs).toEqual({ "10.0.0.99": 444 });
    expect(result.readerClockBaseLocal).toEqual({ "10.0.0.99": 666 });
  });
});
