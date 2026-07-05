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
  formatWallClock,
  computeTickingClock,
  parseWallClock,
  formatHardwareCode,
  formatEpochCreatedAt,
} from "./reader-view-model";

describe("formatHardwareCode", () => {
  it("returns dash for null/undefined", () => {
    expect(formatHardwareCode(null)).toBe("\u2014");
    expect(formatHardwareCode(undefined)).toBe("\u2014");
  });
  it("formats a plain number as hex plus decimal", () => {
    expect(formatHardwareCode(69)).toBe("0x45 (69)");
    expect(formatHardwareCode(0)).toBe("0x0 (0)");
  });
  it("parses decimal strings", () => {
    expect(formatHardwareCode("69")).toBe("0x45 (69)");
  });
  it("parses 0x-prefixed hex strings", () => {
    expect(formatHardwareCode("0x45")).toBe("0x45 (69)");
    expect(formatHardwareCode("0X2A")).toBe("0x2a (42)");
  });
  it("falls back to the raw string when unparseable", () => {
    expect(formatHardwareCode("IPICO")).toBe("IPICO");
    expect(formatHardwareCode("1.5x")).toBe("1.5x");
  });
  it("falls back to the raw value for non-integer or negative numbers", () => {
    expect(formatHardwareCode(1.5)).toBe("1.5");
    expect(formatHardwareCode(-3)).toBe("-3");
  });
  it("returns dash for empty strings", () => {
    expect(formatHardwareCode("")).toBe("\u2014");
    expect(formatHardwareCode("   ")).toBe("\u2014");
  });
});

describe("formatEpochCreatedAt", () => {
  it("returns dash for null/undefined", () => {
    expect(formatEpochCreatedAt(null)).toBe("\u2014");
    expect(formatEpochCreatedAt(undefined)).toBe("\u2014");
  });

  it("formats an epoch creation timestamp as a readable date and time", () => {
    expect(
      formatEpochCreatedAt(Date.UTC(2026, 6, 5, 7, 57), "en-US", "UTC"),
    ).toBe("Jul 5, 2026, 7:57 AM");
  });
});

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

describe("parseWallClock", () => {
  it("parses a space-separated wall clock as UTC", () => {
    expect(parseWallClock("2026-07-01 20:31:13")).toBe(
      Date.UTC(2026, 6, 1, 20, 31, 13),
    );
  });
  it("parses an ISO wall clock with milliseconds as UTC", () => {
    expect(parseWallClock("2026-07-01T20:31:13.500")).toBe(
      Date.UTC(2026, 6, 1, 20, 31, 13, 500),
    );
  });
  it("round-trips through formatWallClock", () => {
    expect(formatWallClock(parseWallClock("2026-07-01 20:31:13"))).toBe(
      "2026-07-01 20:31:13",
    );
  });
  it("returns NaN for unparseable input", () => {
    expect(Number.isNaN(parseWallClock("not a clock"))).toBe(true);
  });
});

describe("formatWallClock", () => {
  it("renders UTC wall-clock fields, not the local timezone", () => {
    // 2026-07-01T20:31:13Z regardless of the machine's timezone.
    const ms = Date.UTC(2026, 6, 1, 20, 31, 13);
    expect(formatWallClock(ms)).toBe("2026-07-01 20:31:13");
  });
});

describe("computeTickingClock", () => {
  const readerBase = Date.UTC(2026, 6, 1, 20, 31, 13);

  it("returns null when the base is unavailable", () => {
    expect(computeTickingClock(null, 1000, 2000)).toBeNull();
    expect(computeTickingClock(readerBase, null, 2000)).toBeNull();
  });

  it("advances the clock by real time elapsed since capture", () => {
    // Captured at baseLocal=1000, now=6000 => +5s.
    expect(computeTickingClock(readerBase, 1000, 6000)).toBe(
      "2026-07-01 20:31:18",
    );
  });

  it("applies the drift offset so the forwarder clock matches when in sync", () => {
    // Reader clock and forwarder clock (offset by +45ms drift) render the
    // same second: proving they line up in the same displayed timezone.
    const reader = computeTickingClock(readerBase, 1000, 1000);
    const forwarder = computeTickingClock(readerBase, 1000, 1000, 45);
    expect(reader).toBe("2026-07-01 20:31:13");
    expect(forwarder).toBe("2026-07-01 20:31:13");
  });

  it("reflects a real drift offset in the derived forwarder clock", () => {
    // +120000ms drift => forwarder clock reads 2 minutes ahead of the reader.
    expect(computeTickingClock(readerBase, 1000, 1000, 120000)).toBe(
      "2026-07-01 20:33:13",
    );
  });
});
