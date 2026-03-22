import { describe, it, expect } from "vitest";
import {
  formatReadMode,
  formatTtoState,
  formatClockDrift,
  driftColorClass,
  computeDownloadPercent,
  formatLastSeen,
  computeTickingLastSeen,
  computeElapsedSecondsSince,
} from "./reader-view-model";

describe("formatReadMode", () => {
  it("returns dash for null/undefined", () => {
    expect(formatReadMode(null)).toBe("\u2014");
    expect(formatReadMode(undefined)).toBe("\u2014");
  });
  it("formats known modes", () => {
    expect(formatReadMode("fsls")).toBe("FS/LS");
    expect(formatReadMode("raw")).toBe("Raw");
    expect(formatReadMode("event")).toBe("Event");
  });
  it("passes through unknown modes", () => {
    expect(formatReadMode("custom")).toBe("custom");
  });
});

describe("formatTtoState", () => {
  it("returns dash for null/undefined", () => {
    expect(formatTtoState(null)).toBe("\u2014");
    expect(formatTtoState(undefined)).toBe("\u2014");
  });
  it("formats enabled/disabled", () => {
    expect(formatTtoState(true)).toBe("Enabled");
    expect(formatTtoState(false)).toBe("Disabled");
  });
});

describe("formatClockDrift", () => {
  it("returns dash for null/undefined", () => {
    expect(formatClockDrift(null)).toBe("\u2014");
    expect(formatClockDrift(undefined)).toBe("\u2014");
  });
  it("formats small positive drift in ms", () => {
    expect(formatClockDrift(42)).toBe("+42ms");
  });
  it("formats small negative drift in ms", () => {
    expect(formatClockDrift(-75)).toBe("-75ms");
  });
  it("formats large drift in seconds", () => {
    expect(formatClockDrift(1500)).toBe("+1.5s");
    expect(formatClockDrift(-2300)).toBe("-2.3s");
  });
  it("formats zero drift", () => {
    expect(formatClockDrift(0)).toBe("+0ms");
  });
  it("formats exactly 1000ms as seconds", () => {
    expect(formatClockDrift(1000)).toBe("+1.0s");
  });
});

describe("driftColorClass", () => {
  it("returns empty for null/undefined", () => {
    expect(driftColorClass(null)).toBe("");
    expect(driftColorClass(undefined)).toBe("");
  });
  it("returns green for drift < 100ms", () => {
    expect(driftColorClass(0)).toBe("text-green-500");
    expect(driftColorClass(99)).toBe("text-green-500");
    expect(driftColorClass(-99)).toBe("text-green-500");
  });
  it("returns yellow for 100ms <= drift < 500ms", () => {
    expect(driftColorClass(100)).toBe("text-yellow-500");
    expect(driftColorClass(499)).toBe("text-yellow-500");
    expect(driftColorClass(-250)).toBe("text-yellow-500");
  });
  it("returns red for drift >= 500ms", () => {
    expect(driftColorClass(500)).toBe("text-red-500");
    expect(driftColorClass(1000)).toBe("text-red-500");
    expect(driftColorClass(-500)).toBe("text-red-500");
  });
});

describe("computeDownloadPercent", () => {
  it("returns 0 for null download", () => {
    expect(computeDownloadPercent(null, null)).toBe(0);
    expect(computeDownloadPercent(undefined, null)).toBe(0);
  });
  it("returns 0 for idle state", () => {
    expect(
      computeDownloadPercent({ state: "idle", reads_received: 0, progress: 0, total: 100 }, null),
    ).toBe(0);
  });
  it("returns 100 for complete state", () => {
    expect(
      computeDownloadPercent({ state: "complete", reads_received: 50, progress: 50, total: 50 }, null),
    ).toBe(100);
  });
  it("returns 0 for error state", () => {
    expect(
      computeDownloadPercent({ state: "error", reads_received: 25, progress: 25, total: 50 }, null),
    ).toBe(0);
  });
  it("uses estimatedReads when available", () => {
    expect(
      computeDownloadPercent({ state: "downloading", reads_received: 50 }, 100),
    ).toBe(50);
  });
  it("falls back to progress/total when no estimatedReads", () => {
    expect(
      computeDownloadPercent({ state: "downloading", progress: 30, total: 60 }, null),
    ).toBe(50);
  });
  it("clamps to 100", () => {
    expect(
      computeDownloadPercent({ state: "downloading", reads_received: 200 }, 100),
    ).toBe(100);
  });
  it("clamps to 0 for negative-ish edge cases", () => {
    expect(
      computeDownloadPercent({ state: "downloading", reads_received: 0 }, 100),
    ).toBe(0);
  });
  it("returns 0 when estimatedReads is 0 and no progress/total", () => {
    expect(
      computeDownloadPercent({ state: "downloading", reads_received: 5 }, 0),
    ).toBe(0);
  });
  it("returns 0 when total is 0", () => {
    expect(
      computeDownloadPercent({ state: "downloading", progress: 0, total: 0 }, null),
    ).toBe(0);
  });
});

describe("formatLastSeen", () => {
  it("returns 'never' for null", () => {
    expect(formatLastSeen(null)).toBe("never");
  });
  it("formats seconds", () => {
    expect(formatLastSeen(30)).toBe("30s ago");
  });
  it("formats minutes", () => {
    expect(formatLastSeen(120)).toBe("2m ago");
  });
  it("formats hours", () => {
    expect(formatLastSeen(7200)).toBe("2h ago");
  });
  it("formats 0 seconds", () => {
    expect(formatLastSeen(0)).toBe("0s ago");
  });
});

describe("computeTickingLastSeen", () => {
  it("returns null for null baseSecs", () => {
    expect(computeTickingLastSeen(null, 1000, 2000)).toBeNull();
  });
  it("returns baseSecs when receivedAt is null", () => {
    expect(computeTickingLastSeen(10, null, 2000)).toBe(10);
  });
  it("adds elapsed seconds", () => {
    expect(computeTickingLastSeen(5, 1000, 4000)).toBe(8); // 5 + 3
  });
  it("never goes negative on elapsed", () => {
    expect(computeTickingLastSeen(5, 4000, 1000)).toBe(5); // max(0, ...)
  });
});

describe("computeElapsedSecondsSince", () => {
  it("computes positive elapsed", () => {
    expect(computeElapsedSecondsSince(1000, 4000)).toBe(3);
  });
  it("never goes negative", () => {
    expect(computeElapsedSecondsSince(4000, 1000)).toBe(0);
  });
});
