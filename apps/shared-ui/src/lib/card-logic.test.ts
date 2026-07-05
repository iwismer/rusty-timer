import { describe, expect, it } from "vitest";
import { resolveHeaderBgClass } from "./card-logic";

describe("resolveHeaderBgClass", () => {
  it("uses status background when borderStatus is set", () => {
    expect(resolveHeaderBgClass("warn", false)).toBe("bg-status-warn-bg");
  });

  it("uses neutral header background when headerBg is true", () => {
    expect(resolveHeaderBgClass(undefined, true)).toBe("bg-surface-2");
  });

  it("uses transparent background when headerBg is false", () => {
    expect(resolveHeaderBgClass(undefined, false)).toBe("");
  });
});
