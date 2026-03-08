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
});

describe("applyReaderInfoUpdate", () => {
  it("ignores late reader info updates for disconnected readers", () => {
    const now = 1234567890;
    const previous = {
      readerInfoMap: {},
      readerInfoReceivedAt: {},
      readerClockBaseTs: {},
      readerClockBaseLocal: {},
      lastReadBase: {},
      lastReadReceivedAt: {},
    };

    const result = applyReaderInfoUpdate(
      makeStatus([
        {
          ip: "10.0.0.42",
          state: "disconnected",
          reads_session: 1,
          reads_total: 2,
          last_read_secs: 3,
          local_port: 10042,
          reader_info: null,
        },
      ]),
      previous,
      {
        ip: "10.0.0.42",
        banner: "late-info",
        clock: { reader_clock: "2026-03-08 12:34:56", drift_ms: 0 },
      },
      now,
    );

    expect(result).toEqual(previous);
  });
});

describe("clearReaderInfoForIp", () => {
  it("removes all reader-info caches for the given IP", () => {
    const previous = {
      readerInfoMap: { "10.0.0.42": makeInfo() },
      readerInfoReceivedAt: { "10.0.0.42": 111 },
      readerClockBaseTs: { "10.0.0.42": 222 },
      readerClockBaseLocal: { "10.0.0.42": 333 },
      lastReadBase: { "10.0.0.42": 5 },
      lastReadReceivedAt: { "10.0.0.42": 444 },
    };
    const result = clearReaderInfoForIp(previous, "10.0.0.42");

    expect(result.readerInfoMap).toEqual({});
    expect(result.readerInfoReceivedAt).toEqual({});
    expect(result.readerClockBaseTs).toEqual({});
    expect(result.readerClockBaseLocal).toEqual({});
    expect(result.lastReadBase).toEqual({ "10.0.0.42": 5 });
    expect(result.lastReadReceivedAt).toEqual({ "10.0.0.42": 444 });
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
      lastReadBase: {},
      lastReadReceivedAt: {},
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
