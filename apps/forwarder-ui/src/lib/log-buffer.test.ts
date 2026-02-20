import { describe, expect, it } from "vitest";
import { pushLogEntry } from "./log-buffer";

describe("pushLogEntry", () => {
  it("appends a new entry", () => {
    expect(pushLogEntry([], "first", 5)).toEqual(["first"]);
  });

  it("keeps only latest max entries", () => {
    expect(pushLogEntry(["a", "b", "c"], "d", 3)).toEqual(["b", "c", "d"]);
  });

  it("trims whitespace-only entries", () => {
    expect(pushLogEntry(["a"], "   ", 5)).toEqual(["a"]);
  });
});
