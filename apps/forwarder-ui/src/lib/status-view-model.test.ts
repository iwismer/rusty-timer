import { describe, expect, it } from "vitest";
import {
  formatLastSeen,
  readerBadgeState,
  readerConnectionSummary,
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
