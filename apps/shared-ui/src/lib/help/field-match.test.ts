import { describe, expect, it } from "vitest";
import { fieldMatchesQuery } from "./field-match";
import type { FieldHelp } from "./help-types";

const field: FieldHelp = {
  label: "Base URL",
  summary: "WebSocket server address.",
  detail: "The full URL including protocol.",
  default: "ws://localhost:8080",
  range: "Valid URL",
  recommended: "Use wss:// in production",
};

describe("fieldMatchesQuery", () => {
  it("matches on label", () => {
    expect(fieldMatchesQuery(field, "base url")).toBe(true);
  });
  it("matches on summary", () => {
    expect(fieldMatchesQuery(field, "websocket")).toBe(true);
  });
  it("matches on detail", () => {
    expect(fieldMatchesQuery(field, "protocol")).toBe(true);
  });
  it("matches on default", () => {
    expect(fieldMatchesQuery(field, "localhost")).toBe(true);
  });
  it("matches on range", () => {
    expect(fieldMatchesQuery(field, "valid url")).toBe(true);
  });
  it("matches on recommended", () => {
    expect(fieldMatchesQuery(field, "wss")).toBe(true);
  });
  it("is case-insensitive", () => {
    expect(fieldMatchesQuery(field, "BASE URL")).toBe(true);
  });
  it("returns false for non-match", () => {
    expect(fieldMatchesQuery(field, "zzz-no-match")).toBe(false);
  });
  it("handles field with no optional properties", () => {
    const minimal: FieldHelp = { label: "X", summary: "Y", detail: "Z" };
    expect(fieldMatchesQuery(minimal, "x")).toBe(true);
    expect(fieldMatchesQuery(minimal, "zzz")).toBe(false);
  });
});
