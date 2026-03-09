import { describe, expect, it } from "vitest";
import { resolvePopoverPosition } from "./help-tip";

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
