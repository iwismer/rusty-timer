import { describe, expect, it } from "vitest";

import {
  READ_MODE_OPTIONS,
  initialTimeoutDraft,
  resolveTimeoutSeconds,
  shouldShowTimeoutInput,
} from "./read-mode-form";

describe("read mode form helpers", () => {
  it("includes raw, event, and fsls read mode options", () => {
    expect(READ_MODE_OPTIONS).toEqual([
      { value: "raw", label: "Raw" },
      { value: "event", label: "Event" },
      { value: "fsls", label: "First/Last Seen" },
    ]);
  });

  it("shows timeout input for event and fsls", () => {
    expect(shouldShowTimeoutInput("event")).toBe(true);
    expect(shouldShowTimeoutInput("fsls")).toBe(true);
    expect(shouldShowTimeoutInput("raw")).toBe(false);
    expect(shouldShowTimeoutInput(undefined)).toBe(false);
  });

  it("uses the current reader timeout when available", () => {
    expect(initialTimeoutDraft(12)).toBe("12");
    expect(initialTimeoutDraft(null)).toBe("5");
    expect(initialTimeoutDraft(undefined)).toBe("5");
  });

  it("parses and clamps timeout to a sane range", () => {
    expect(resolveTimeoutSeconds("10", 5)).toBe(10);
    expect(resolveTimeoutSeconds("", 5)).toBe(5);
    expect(resolveTimeoutSeconds("0", 5)).toBe(1);
    expect(resolveTimeoutSeconds("999", 5)).toBe(255);
  });

  it("falls back to the current timeout when the draft is blank", () => {
    expect(resolveTimeoutSeconds("", 12)).toBe(12);
  });
});
