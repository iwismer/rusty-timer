import { describe, expect, it } from "vitest";
import { mapSaveSectionResult } from "./config-api-adapter";

describe("mapSaveSectionResult", () => {
  it("returns restart_needed=false for failed saves", () => {
    expect(mapSaveSectionResult({ ok: false, error: "boom" })).toEqual({
      ok: false,
      error: "boom",
      restart_needed: false,
    });
  });

  it("returns restart_needed=true when server says restart_needed=true", () => {
    expect(mapSaveSectionResult({ ok: true, restart_needed: true })).toEqual({
      ok: true,
      error: undefined,
      restart_needed: true,
    });
  });

  it("returns restart_needed=false when server says restart_needed=false", () => {
    expect(mapSaveSectionResult({ ok: true, restart_needed: false })).toEqual({
      ok: true,
      error: undefined,
      restart_needed: false,
    });
  });

  it("defaults restart_needed=true when field absent on success", () => {
    expect(mapSaveSectionResult({ ok: true })).toEqual({
      ok: true,
      error: undefined,
      restart_needed: true,
    });
  });

  it("returns restart_needed=false when not ok even if server says true", () => {
    expect(mapSaveSectionResult({ ok: false, restart_needed: true })).toEqual({
      ok: false,
      error: undefined,
      restart_needed: false,
    });
  });
});
