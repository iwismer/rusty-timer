import { describe, expect, it } from "vitest";
import {
  computeElapsedSecondsSince,
  formatLastSeen,
  readerBadgeState,
  readerConnectionSummary,
  formatClockDrift,
  formatReadMode,
  formatTtoState,
  readerControlDisabled,
  computeDownloadPercent,
  computeTickingLastSeen,
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

describe("formatReadMode — event", () => {
  it("capitalizes event mode", () => {
    expect(formatReadMode("event")).toBe("Event");
  });
});

describe("formatTtoState", () => {
  it("renders enabled, disabled, and unknown states", () => {
    expect(formatTtoState(true)).toBe("Enabled");
    expect(formatTtoState(false)).toBe("Disabled");
    expect(formatTtoState(null)).toBe("\u2014");
  });
});

describe("readerControlDisabled", () => {
  it("disables controls while busy", () => {
    expect(readerControlDisabled("connected", true)).toBe(true);
  });

  it("disables controls while disconnected", () => {
    expect(readerControlDisabled("disconnected", false)).toBe(true);
    expect(readerControlDisabled("connecting", false)).toBe(true);
  });

  it("keeps controls enabled when connected and idle", () => {
    expect(readerControlDisabled("connected", false)).toBe(false);
  });
});

describe("computeTickingLastSeen", () => {
  it("returns null when base is null", () => {
    expect(computeTickingLastSeen(null, 1000, 2000)).toBe(null);
  });

  it("returns base when receivedAt is null", () => {
    expect(computeTickingLastSeen(5, null, 2000)).toBe(5);
  });

  it("adds elapsed seconds to base", () => {
    expect(computeTickingLastSeen(5, 10000, 13000)).toBe(8);
  });

  it("floors partial seconds", () => {
    expect(computeTickingLastSeen(5, 10000, 12999)).toBe(7);
  });

  it("handles zero base", () => {
    expect(computeTickingLastSeen(0, 10000, 11500)).toBe(1);
  });

  it("clamps elapsed to zero when now is before receivedAt", () => {
    expect(computeTickingLastSeen(2, 10000, 9500)).toBe(2);
  });
});

describe("computeElapsedSecondsSince", () => {
  it("clamps negative elapsed time to zero", () => {
    expect(computeElapsedSecondsSince(10_600, 10_000)).toBe(0);
  });

  it("rounds positive elapsed time to the nearest second", () => {
    expect(computeElapsedSecondsSince(10_000, 10_600)).toBe(1);
  });
});
