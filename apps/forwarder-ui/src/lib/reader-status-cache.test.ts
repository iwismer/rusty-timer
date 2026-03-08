import { describe, expect, it } from "vitest";
import type { ForwarderStatus, ReaderInfo } from "./api";
import { rebuildReaderCachesFromStatus } from "./reader-status-cache";

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
