import { describe, expect, it } from "vitest";
import { shouldCancelOnBackdropClick, shouldCancelOnEscape } from "./confirm-dialog";

describe("shouldCancelOnBackdropClick", () => {
  it("returns true when click target is the dialog element", () => {
    const dialog = {};
    expect(shouldCancelOnBackdropClick(dialog, dialog)).toBe(true);
  });

  it("returns false when click target is an inner element", () => {
    const dialog = {};
    const inner = {};
    expect(shouldCancelOnBackdropClick(inner, dialog)).toBe(false);
  });
});

describe("shouldCancelOnEscape", () => {
  it("returns true for Escape", () => {
    expect(shouldCancelOnEscape("Escape")).toBe(true);
  });

  it("returns false for other keys", () => {
    expect(shouldCancelOnEscape("Enter")).toBe(false);
  });
});
