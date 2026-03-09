import { describe, expect, it } from "vitest";
import { computePopoverStyle, resolvePopoverPosition } from "./help-tip";

describe("computePopoverStyle", () => {
  const btn = { top: 100, bottom: 120, left: 50, right: 66 };

  it("positions below the button by default", () => {
    const style = computePopoverStyle(btn, 1024, 800);
    expect(style).toContain("top: 128px"); // bottom(120) + gap(8)
    expect(style).toContain("left: 50px");
  });

  it("positions above the button when near viewport bottom", () => {
    const nearBottom = { top: 700, bottom: 720, left: 50, right: 66 };
    const style = computePopoverStyle(nearBottom, 1024, 800);
    expect(style).toContain("top: 492px"); // top(700) - 200 - 8
  });

  it("clamps left edge when popover would overflow right side", () => {
    const nearRight = { top: 100, bottom: 120, left: 800, right: 816 };
    const style = computePopoverStyle(nearRight, 400, 800);
    // 400 - 288 - 8 = 104
    expect(style).toContain("left: 104px");
  });

  it("clamps left edge to minimum gap", () => {
    const nearLeft = { top: 100, bottom: 120, left: 2, right: 18 };
    const style = computePopoverStyle(nearLeft, 1024, 800);
    expect(style).toContain("left: 8px");
  });
});

describe("resolvePopoverPosition", () => {
  it("returns 'below' when there is plenty of space below", () => {
    const result = resolvePopoverPosition({ top: 100, bottom: 120 }, 800);
    expect(result).toBe("below");
  });

  it("returns 'above' when near viewport bottom with space above", () => {
    const result = resolvePopoverPosition({ top: 700, bottom: 720 }, 800);
    expect(result).toBe("above");
  });

  it("returns 'below' when near both top and bottom (default)", () => {
    const result = resolvePopoverPosition({ top: 50, bottom: 70 }, 200);
    expect(result).toBe("below");
  });

  it("returns 'below' with custom small popover height that fits", () => {
    const result = resolvePopoverPosition({ top: 100, bottom: 120 }, 600, 100);
    expect(result).toBe("below");
  });

  it("returns 'above' with custom popover height when bottom is tight", () => {
    const result = resolvePopoverPosition({ top: 500, bottom: 520 }, 600, 300);
    expect(result).toBe("above");
  });
});
