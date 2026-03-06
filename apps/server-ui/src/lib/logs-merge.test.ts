import { describe, expect, it } from "vitest";
import { mergeLogsWithPendingLive } from "./logs-merge";

describe("mergeLogsWithPendingLive", () => {
  it("prepends live entries emitted during in-flight resync", () => {
    const snapshot = ["b", "a"];
    const pending = ["c"];
    expect(mergeLogsWithPendingLive(snapshot, pending, 500)).toEqual([
      "c",
      "b",
      "a",
    ]);
  });

  it("does not duplicate entries already in snapshot", () => {
    const snapshot = ["b", "a"];
    const pending = ["b", "c"];
    expect(mergeLogsWithPendingLive(snapshot, pending, 500)).toEqual([
      "c",
      "b",
      "a",
    ]);
  });

  it("enforces max retention trimming from end", () => {
    const snapshot = Array.from({ length: 500 }, (_, i) => `s-${i}`);
    const pending = ["live-1", "live-2"];
    const merged = mergeLogsWithPendingLive(snapshot, pending, 500);
    expect(merged).toHaveLength(500);
    expect(merged[0]).toBe("live-2");
    expect(merged[1]).toBe("live-1");
  });
});
