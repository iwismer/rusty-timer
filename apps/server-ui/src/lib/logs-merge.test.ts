import { describe, expect, it } from "vitest";
import { mergeLogsWithPendingLive } from "./logs-merge";

describe("mergeLogsWithPendingLive", () => {
  it("preserves live entries emitted during in-flight resync", () => {
    const snapshot = ["a", "b"];
    const pending = ["c"];
    expect(mergeLogsWithPendingLive(snapshot, pending, 500)).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  it("does not duplicate entries already in snapshot", () => {
    const snapshot = ["a", "b"];
    const pending = ["b", "c"];
    expect(mergeLogsWithPendingLive(snapshot, pending, 500)).toEqual([
      "a",
      "b",
      "c",
    ]);
  });

  it("enforces max retention", () => {
    const snapshot = Array.from({ length: 500 }, (_, i) => `s-${i}`);
    const pending = ["live-1", "live-2"];
    const merged = mergeLogsWithPendingLive(snapshot, pending, 500);
    expect(merged).toHaveLength(500);
    expect(merged.at(-1)).toBe("live-2");
  });
});
