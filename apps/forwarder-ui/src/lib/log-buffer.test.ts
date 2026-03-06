import { describe, expect, it } from "vitest";
import { pushLogEntry } from "./log-buffer";

describe("pushLogEntry", () => {
  it("prepends a new entry", () => {
    expect(pushLogEntry([], "first", 5)).toEqual(["first"]);
  });

  it("prepends newest entry to front", () => {
    expect(pushLogEntry(["b", "a"], "c", 5)).toEqual(["c", "b", "a"]);
  });

  it("keeps only latest max entries, trimming from end", () => {
    expect(pushLogEntry(["c", "b", "a"], "d", 3)).toEqual(["d", "c", "b"]);
  });

  it("trims whitespace-only entries", () => {
    expect(pushLogEntry(["a"], "   ", 5)).toEqual(["a"]);
  });

  it("preserves 500-entry retention after initial snapshot", () => {
    const initial = Array.from({ length: 500 }, (_, i) => `e-${i}`);
    const next = pushLogEntry(initial, "live");
    expect(next).toHaveLength(500);
    expect(next[0]).toBe("live");
  });
});
