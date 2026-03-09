import { describe, expect, it } from "vitest";
import { computePopoverStyle } from "./help-tip";

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

  it("defaults to below when neither above nor below has sufficient space", () => {
    const rect = { top: 100, bottom: 120, left: 50, right: 70 };
    // viewport only 200px tall: 80px above, 80px below — neither has 200+8 space
    const style = computePopoverStyle(rect, 800, 200);
    // Should default to below: top = bottom + GAP = 128
    expect(style).toContain("top: 128px");
  });

  it("clamps left edge to minimum gap", () => {
    const nearLeft = { top: 100, bottom: 120, left: 2, right: 18 };
    const style = computePopoverStyle(nearLeft, 1024, 800);
    expect(style).toContain("left: 8px");
  });
});
