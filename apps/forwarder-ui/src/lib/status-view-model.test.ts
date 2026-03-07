import { describe, expect, it } from "vitest";
import {
  formatLastSeen,
  readerBadgeState,
  readerBorderStatus,
  readerConnectionSummary,
  formatClockDrift,
  formatReadMode,
  computeDownloadPercent,
} from "./status-view-model";

describe("formatLastSeen", () => {
  it("formats null as never", () => {
    expect(formatLastSeen(null)).toBe("never");
  });

  it("formats seconds/minutes/hours", () => {
    expect(formatLastSeen(12)).toBe("12s ago");
    expect(formatLastSeen(125)).toBe("2m ago");
    expect(formatLastSeen(7200)).toBe("2h ago");
  });
});

describe("readerBadgeState", () => {
  it("maps reader state to badge state", () => {
    expect(readerBadgeState("connected")).toBe("ok");
    expect(readerBadgeState("connecting")).toBe("warn");
    expect(readerBadgeState("disconnected")).toBe("err");
  });
});

describe("readerConnectionSummary", () => {
  it("returns connected/configured summary", () => {
    const summary = readerConnectionSummary([
      {
        ip: "a",
        state: "connected",
        reads_session: 0,
        reads_total: 0,
        last_seen_secs: null,
        local_port: 10001,
      },
      {
        ip: "b",
        state: "disconnected",
        reads_session: 0,
        reads_total: 0,
        last_seen_secs: null,
        local_port: 10002,
      },
      {
        ip: "c",
        state: "connected",
        reads_session: 0,
        reads_total: 0,
        last_seen_secs: null,
        local_port: 10003,
      },
    ]);

    expect(summary).toEqual({
      connected: 2,
      configured: 3,
      label: "2 connected / 3 configured",
    });
  });
});

describe("formatReadMode", () => {
  it("formats known modes", () => {
    expect(formatReadMode("fsls")).toBe("FS/LS");
    expect(formatReadMode("raw")).toBe("Raw");
  });

  it("returns em dash for null/undefined", () => {
    expect(formatReadMode(null)).toBe("\u2014");
    expect(formatReadMode(undefined)).toBe("\u2014");
  });

  it("passes through unknown modes", () => {
    expect(formatReadMode("other")).toBe("other");
  });
});

describe("formatClockDrift", () => {
  it("formats milliseconds", () => {
    expect(formatClockDrift(null)).toBe("\u2014");
    expect(formatClockDrift(undefined)).toBe("\u2014");
    expect(formatClockDrift(50)).toBe("+50ms");
    expect(formatClockDrift(-200)).toBe("-200ms");
    expect(formatClockDrift(1500)).toBe("+1.5s");
    expect(formatClockDrift(-3200)).toBe("-3.2s");
  });
});

describe("computeDownloadPercent", () => {
  it("uses reads_received against estimated reads when available", () => {
    expect(
      computeDownloadPercent(
        { state: "downloading", reads_received: 40, progress: 0, total: 3200 },
        100,
      ),
    ).toBe(40);
  });

  it("falls back to progress/total when estimated reads unavailable", () => {
    expect(
      computeDownloadPercent(
        { state: "downloading", reads_received: 5, progress: 25, total: 50 },
        null,
      ),
    ).toBe(50);
  });

  it("clamps to 100", () => {
    expect(
      computeDownloadPercent(
        { state: "downloading", reads_received: 120, progress: 0, total: 5000 },
        100,
      ),
    ).toBe(100);
  });
});

describe("readerBorderStatus", () => {
  it("maps reader state to card border status", () => {
    expect(readerBorderStatus("connected")).toBe("ok");
    expect(readerBorderStatus("connecting")).toBe("warn");
    expect(readerBorderStatus("disconnected")).toBe("err");
  });
});
