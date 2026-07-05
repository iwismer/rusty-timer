import { describe, it, expect } from "vitest";
import {
  shouldShowTimeoutInput,
  initialTimeoutDraft,
  resolveTimeoutSeconds,
} from "./read-mode-form";

describe("shouldShowTimeoutInput", () => {
  it("returns true for event mode", () => {
    expect(shouldShowTimeoutInput("event")).toBe(true);
  });
  it("returns true for fsls mode", () => {
    expect(shouldShowTimeoutInput("fsls")).toBe(true);
  });
  it("returns false for raw mode", () => {
    expect(shouldShowTimeoutInput("raw")).toBe(false);
  });
  it("returns false for null/undefined", () => {
    expect(shouldShowTimeoutInput(null)).toBe(false);
    expect(shouldShowTimeoutInput(undefined)).toBe(false);
  });
});

describe("initialTimeoutDraft", () => {
  it("returns current value as string when finite", () => {
    expect(initialTimeoutDraft(10)).toBe("10");
  });
  it("returns default for null/undefined", () => {
    expect(initialTimeoutDraft(null)).toBe("5");
    expect(initialTimeoutDraft(undefined)).toBe("5");
  });
  it("returns default for non-finite values", () => {
    expect(initialTimeoutDraft(NaN)).toBe("5");
    expect(initialTimeoutDraft(Infinity)).toBe("5");
  });
});

describe("resolveTimeoutSeconds", () => {
  it("parses valid integer", () => {
    expect(resolveTimeoutSeconds("10", null)).toBe(10);
  });
  it("clamps to minimum of 1", () => {
    expect(resolveTimeoutSeconds("0", null)).toBe(1);
    expect(resolveTimeoutSeconds("-5", null)).toBe(1);
  });
  it("clamps to maximum of 255", () => {
    expect(resolveTimeoutSeconds("300", null)).toBe(255);
  });
  it("uses fallback when draft is empty", () => {
    expect(resolveTimeoutSeconds("", 15)).toBe(15);
  });
  it("uses fallback when draft is non-numeric", () => {
    expect(resolveTimeoutSeconds("abc", 20)).toBe(20);
  });
  it("returns default when both draft and fallback are invalid", () => {
    expect(resolveTimeoutSeconds("abc", null)).toBe(5);
    expect(resolveTimeoutSeconds("", undefined)).toBe(5);
  });
  it("clamps fallback too", () => {
    expect(resolveTimeoutSeconds("", 999)).toBe(255);
    expect(resolveTimeoutSeconds("", 0)).toBe(1);
  });
  it("handles whitespace in draft", () => {
    expect(resolveTimeoutSeconds("  42  ", null)).toBe(42);
  });
});
